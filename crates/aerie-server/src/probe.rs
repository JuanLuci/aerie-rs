//! Probe: joins a server in insecure mode as a minimal client, goes through
//! setup, finishes it, then listens for a while. Every packet the server
//! sends, on the game stream and on the streams it opens, is saved. Used to
//! study a server; it never plays.

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use aerie_protocol::client::{ConnectOut, InsecurePlayerOptionsOut, RequestAssetsOut};
use aerie_protocol::packets::{ClientType, Outgoing};
use aerie_protocol::wire::Wire;
use aerie_protocol::{Frame, FrameDecoder, frame, proto, table};
use anyhow::{Context, Result, bail};
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use crate::tls;

/// How long to keep listening after `SetupFinalize`.
const JOIN_LISTEN: Duration = Duration::from_secs(20);

struct Recorder {
    dir: PathBuf,
    seq: u32,
    index: Vec<String>,
}

impl Recorder {
    async fn save(rec: &Mutex<Recorder>, stream: &str, f: &Frame) -> Result<()> {
        let mut r = rec.lock().await;
        let seq = r.seq;
        r.seq += 1;
        let name = table::info(f.id).map_or("?", |i| i.name);
        info!(
            "← [{stream}] #{seq} {} {name} {} bytes",
            f.id,
            f.payload.len()
        );
        tokio::fs::write(
            r.dir.join(format!("{seq:04}-{}-{name}.bin", f.id)),
            &f.payload,
        )
        .await?;
        let line = format!("{seq}\t{}\t{name}\t{}\t{stream}", f.id, f.payload.len());
        // Written as it goes, so a visit that ends in an error keeps its index.
        {
            use tokio::io::AsyncWriteExt;
            let mut ix = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(r.dir.join("index.tsv"))
                .await?;
            ix.write_all(format!("{line}\n").as_bytes()).await?;
        }
        r.index.push(line);
        Ok(())
    }
}

/// What the probe does besides listening.
#[derive(Default)]
pub struct Options {
    pub name: Option<String>,
    /// Chat lines to send once in the world (`;` separates lines).
    pub chat: Option<String>,
    /// Packets to send after the chat lines: `id:hex;id:hex`.
    pub send: Option<String>,
    /// A folder of recorded client packets (`--record`): its interaction
    /// chains are sent again once in the world, moved to the block under the
    /// probe's spawn, to see how a server answers them.
    pub interactions: Option<PathBuf>,
}

/// The interaction chains of a `--record` folder, in order.
fn recorded_chains(dir: &std::path::Path) -> Result<Vec<proto::SyncInteractionChains>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().contains("-290-"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|f| Ok(proto::SyncInteractionChains::decode(&std::fs::read(f)?, 0)?.0))
        .collect()
}

/// Moves every block position of a chain (and its forks) by `d`.
fn shift(c: &mut proto::SyncInteractionChain, d: (i32, i32, i32)) {
    let mv = |p: &mut proto::BlockPosition| {
        p.x += d.0;
        p.y += d.1;
        p.z += d.2;
    };
    if let Some(p) = c.data.as_mut().and_then(|x| x.block_position.as_mut()) {
        mv(p);
    }
    for s in c.interaction_data.iter_mut().flatten().flatten() {
        if let Some(p) = s.block_position.as_mut() {
            mv(p);
        }
    }
    for f in c.new_forks.iter_mut().flatten() {
        shift(f, d);
    }
}

pub async fn run(upstream: SocketAddr, capture: PathBuf, opts: Options) -> Result<()> {
    tokio::fs::create_dir_all(&capture).await?;
    let rec = Arc::new(Mutex::new(Recorder {
        dir: capture.clone(),
        seq: 0,
        index: Vec::new(),
    }));
    let mut ep = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    ep.set_default_client_config(tls::client_config()?);
    let conn = ep
        .connect(upstream, "localhost")?
        .await
        .context("connecting")?;
    let (mut send_stream, mut recv) = conn.open_bi().await?;
    // One task writes the game stream, so sending (a slow replay of
    // interactions, say) never stops the reading below.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    tokio::spawn(async move {
        while let Some(w) = out_rx.recv().await {
            if send_stream.write_all(&w).await.is_err() {
                break;
            }
        }
    });

    // Streams the server opens (Chunks, WorldMap) are recorded as they come.
    {
        let (conn, rec) = (conn.clone(), rec.clone());
        tokio::spawn(async move {
            let mut n = 0;
            while let Ok(mut s) = conn.accept_uni().await {
                n += 1;
                let (rec, tag) = (rec.clone(), format!("uni{n}"));
                tokio::spawn(async move {
                    let mut dec = FrameDecoder::new(false);
                    let mut buf = vec![0u8; 256 * 1024];
                    while let Ok(Some(k)) = s.read(&mut buf).await {
                        dec.push(&buf[..k]);
                        loop {
                            match dec.next() {
                                Ok(Some(f)) => {
                                    let _ = Recorder::save(&rec, &tag, &f).await;
                                }
                                Ok(None) => break,
                                Err(e) => {
                                    warn!("[{tag}] cannot decode: {e}");
                                    return;
                                }
                            }
                        }
                    }
                });
            }
        });
    }

    let mut wire = Vec::new();
    ConnectOut {
        client_version: "0.7.7".into(),
        client_type: ClientType::Game,
        identity_token: None,
        language: "en-US".into(),
    }
    .write_frame(&mut wire)?;
    let _ = out_tx.send(wire.clone());

    let mut dec = FrameDecoder::new(false);
    let mut buf = vec![0u8; 256 * 1024];
    let mut join_deadline: Option<tokio::time::Instant> = None;
    let mut client_id: Option<i32> = None;
    let mut chains = match &opts.interactions {
        Some(d) => recorded_chains(d)?,
        None => Vec::new(),
    };
    loop {
        let f = loop {
            if let Some(f) = dec.next()? {
                break Some(f);
            }
            let wait = match join_deadline {
                Some(d) => d.saturating_duration_since(tokio::time::Instant::now()),
                None => Duration::from_secs(120),
            };
            match tokio::time::timeout(wait, recv.read(&mut buf)).await {
                Ok(r) => {
                    let n = r?.context("server closed the stream")?;
                    dec.push(&buf[..n]);
                }
                Err(_) if join_deadline.is_some() => break None,
                Err(_) => bail!("server went quiet"),
            }
        };
        let Some(f) = f else { break };
        Recorder::save(&rec, "game", &f).await?;

        let mut wire = Vec::new();
        match f.id {
            364 => InsecurePlayerOptionsOut {
                uuid: Uuid::new_v4(),
                username: opts.name.clone().unwrap_or_else(|| "AerieProbe".into()),
            }
            .write_frame(&mut wire)?,
            20 => RequestAssetsOut { assets: Vec::new() }.write_frame(&mut wire)?,
            2 => {
                warn!("server disconnected the probe");
                break;
            }
            22 => {
                // Finish setup the way the client does: view radius, then SetupFinalize.
                frame::encode(32, &224i32.to_le_bytes(), &mut wire)?;
                frame::encode(33, &[], &mut wire)?;
                join_deadline = Some(tokio::time::Instant::now() + JOIN_LISTEN);
            }
            3 if f.payload.len() >= 16 => {
                // Answer pings: same id and time, type Raw, empty queue.
                let mut pong = f.payload[..16].to_vec();
                pong.push(0);
                pong.extend_from_slice(&0i16.to_le_bytes());
                frame::encode(4, &pong, &mut wire)?;
            }
            104 => {
                // JoinWorld: ready for chunks now, ready for gameplay right after.
                frame::encode(105, &[0b01], &mut wire)?;
                frame::encode(105, &[0b10], &mut wire)?;
                // Unless replaying interactions (which must not move the
                // probe): a position, then the chat lines, so a server can
                // be tested past the join.
                if opts.interactions.is_none() {
                    let mut payload = Vec::new();
                    proto::ClientMovement {
                        absolute_position: Some(proto::Position {
                            x: 1.5,
                            y: 80.0,
                            z: -3.0,
                        }),
                        ..Default::default()
                    }
                    .encode(&mut payload)?;
                    frame::encode(108, &payload, &mut wire)?;
                }
                // Chat lines and extra packets once the server has put the
                // player in the world (it drops them before), one by one.
                let mut later = Vec::new();
                for line in opts.chat.iter().flat_map(|c| c.split(';')) {
                    let mut payload = Vec::new();
                    proto::ChatMessage {
                        message: Some(line.trim().to_owned()),
                    }
                    .encode(&mut payload)?;
                    let mut w = Vec::new();
                    frame::encode(211, &payload, &mut w)?;
                    later.push(w);
                }
                for p in opts.send.iter().flat_map(|c| c.split(';')) {
                    let Some((id, hex)) = p.trim().split_once(':') else {
                        continue;
                    };
                    let bytes: Vec<u8> = (0..hex.len() / 2)
                        .filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
                        .collect();
                    let mut w = Vec::new();
                    frame::encode(id.parse()?, &bytes, &mut w)?;
                    later.push(w);
                }
                if !later.is_empty() {
                    let tx = out_tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(4)).await;
                        for w in later {
                            if tx.send(w).is_err() {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(700)).await;
                        }
                    });
                }
            }
            100 => {
                client_id = proto::SetClientId::decode(&f.payload, 0)
                    .ok()
                    .map(|(c, _)| c.client_id);
                if !chains.is_empty() {
                    // The interactions take a while: listen longer.
                    join_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(90));
                }
            }
            161 if !chains.is_empty() => {
                // Our own entity's position: replay the chains on the block under it.
                let at = proto::EntityUpdates::decode(&f.payload, 0)
                    .ok()
                    .and_then(|(u, _)| {
                        u.updates?
                            .into_iter()
                            .find(|e| Some(e.network_id) == client_id)?
                            .updates?
                            .into_iter()
                            .find_map(|c| match c {
                                proto::ComponentUpdate::TransformUpdate(t) => t.transform.position,
                                _ => None,
                            })
                    });
                if let Some(p) = at {
                    let target = (
                        p.x.floor() as i32,
                        p.y.floor() as i32 - 1,
                        p.z.floor() as i32,
                    );
                    let first = chains
                        .iter()
                        .flat_map(|m| &m.updates)
                        .find_map(|c| c.data.as_ref()?.block_position.clone());
                    if let Some(first) = first {
                        let d = (target.0 - first.x, target.1 - first.y, target.2 - first.z);
                        info!(
                            "replaying {} interaction packets at {target:?}",
                            chains.len()
                        );
                        let list = std::mem::take(&mut chains);
                        let tx = out_tx.clone();
                        tokio::spawn(async move {
                            for mut m in list {
                                for c in &mut m.updates {
                                    shift(c, d);
                                }
                                let mut payload = Vec::new();
                                let mut w = Vec::new();
                                if m.encode(&mut payload).is_err()
                                    || frame::encode(290, &payload, &mut w).is_err()
                                    || tx.send(w).is_err()
                                {
                                    break;
                                }
                                tokio::time::sleep(Duration::from_millis(60)).await;
                            }
                        });
                    }
                }
            }
            210 => {
                if let Ok((m, _)) = proto::ServerMessage::decode(&f.payload, 0) {
                    let text = m.message.and_then(|m| m.raw_text.or(m.message_id));
                    info!("chat: {}", text.unwrap_or_default());
                }
            }
            _ => {}
        }
        if !wire.is_empty() {
            let _ = out_tx.send(wire);
        }
    }
    let r = rec.lock().await;
    info!("captured {} packets into {}", r.seq, capture.display());
    conn.close(0u32.into(), b"");
    Ok(())
}
