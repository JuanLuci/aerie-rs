//! Capture proxy: sits between a client and a server, relays every stream
//! untouched and logs each frame (direction, id, name, size). Payloads up to
//! 1 MiB are saved to the capture folder for study.

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
};

use aerie_protocol::{FrameDecoder, table};
use anyhow::Result;
use tracing::{info, warn};

use crate::tls;

pub async fn run(bind: SocketAddr, upstream: SocketAddr, capture: PathBuf) -> Result<()> {
    tokio::fs::create_dir_all(&capture).await?;
    let mut client_side = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    client_side.set_default_client_config(tls::client_config()?);
    let listener = quinn::Endpoint::server(tls::server_config()?.0, bind)?;
    info!(
        "proxy listening on {bind}, relaying to {upstream}, captures in {}",
        capture.display()
    );

    let session = Arc::new(AtomicU64::new(0));
    while let Some(incoming) = listener.accept().await {
        let (client_side, capture, session) =
            (client_side.clone(), capture.clone(), session.clone());
        tokio::spawn(async move {
            let n = session.fetch_add(1, Ordering::Relaxed);
            if let Err(e) = relay(
                incoming,
                client_side,
                upstream,
                capture.join(format!("session-{n}")),
            )
            .await
            {
                warn!("session {n} ended: {e:#}");
            }
        });
    }
    Ok(())
}

async fn relay(
    incoming: quinn::Incoming,
    client_side: quinn::Endpoint,
    upstream: SocketAddr,
    dir: PathBuf,
) -> Result<()> {
    tokio::fs::create_dir_all(&dir).await?;
    let down = incoming.await?;
    info!(
        "client {} connected, ALPN {:?}",
        down.remote_address(),
        tls::negotiated_alpn(&down).map(String::from_utf8)
    );
    let up = client_side.connect(upstream, "localhost")?.await?;
    info!("upstream connected");
    let seq = Arc::new(AtomicU64::new(0));

    let mut stream_no = 0;
    loop {
        tokio::select! {
            s = down.accept_bi() => {
                let (c_send, c_recv) = s?;
                let (s_send, s_recv) = up.open_bi().await?;
                let tag = stream_no;
                stream_no += 1;
                info!("stream {tag} opened by client");
                tokio::spawn(pipe(c_recv, s_send, true, tag, dir.clone(), seq.clone()));
                tokio::spawn(pipe(s_recv, c_send, false, tag, dir.clone(), seq.clone()));
            }
            s = up.accept_bi() => {
                let (s_send, s_recv) = s?;
                let (c_send, c_recv) = down.open_bi().await?;
                let tag = stream_no;
                stream_no += 1;
                info!("stream {tag} opened by server");
                tokio::spawn(pipe(s_recv, c_send, false, tag, dir.clone(), seq.clone()));
                tokio::spawn(pipe(c_recv, s_send, true, tag, dir.clone(), seq.clone()));
            }
            e = down.closed() => { info!("client closed: {e}"); up.close(0u32.into(), b""); return Ok(()); }
            e = up.closed() => {
                info!("server closed: {e}");
                if let quinn::ConnectionError::ApplicationClosed(c) = &e {
                    down.close(c.error_code, &c.reason);
                }
                return Ok(());
            }
        }
    }
}

async fn pipe(
    mut from: quinn::RecvStream,
    mut to: quinn::SendStream,
    to_server: bool,
    stream: u32,
    dir: PathBuf,
    seq: Arc<AtomicU64>,
) {
    let arrow = if to_server { "C→S" } else { "S→C" };
    let mut dec = FrameDecoder::new(to_server);
    let mut broken = false;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(Some(n)) => n,
            Ok(None) => break,
            Err(e) => {
                info!("[s{stream} {arrow}] read ended: {e}");
                break;
            }
        };
        if to.write_all(&buf[..n]).await.is_err() {
            break;
        }
        if broken {
            continue;
        }
        dec.push(&buf[..n]);
        loop {
            match dec.next() {
                Ok(Some(f)) => {
                    let k = seq.fetch_add(1, Ordering::Relaxed);
                    let name = table::info(f.id).map_or("?", |i| i.name);
                    let preview: String = f
                        .payload
                        .iter()
                        .take(48)
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    info!(
                        "[s{stream} {arrow}] #{k} {} {name} {} bytes  {preview}",
                        f.id,
                        f.payload.len()
                    );
                    if f.payload.len() <= 1 << 20 {
                        let side = if to_server { "c2s" } else { "s2c" };
                        let _ = tokio::fs::write(
                            dir.join(format!("{k:05}-s{stream}-{side}-{}-{name}.bin", f.id)),
                            &f.payload,
                        )
                        .await;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    warn!("[s{stream} {arrow}] cannot decode: {e}; relaying raw from here on");
                    broken = true;
                    break;
                }
            }
        }
    }
    let _ = to.finish();
}
