//! Aerie dedicated server.
//!
//! ```text
//! aerie-server login [--profile NAME] [--account FILE]
//! aerie-server fetch-game [--out DIR] [--patchline NAME] [--list] [--keep-package] [--account FILE]
//! aerie-server [--bind 0.0.0.0:5520] [--password PW] [--name NAME] [--account FILE]
//!              [--config FILE] [--motd TEXT] [--max-players N] [--max-view-radius N]
//!              [--game-mode adventure|creative]
//! aerie-server probe --upstream ADDR [--capture DIR] [--name NAME] [--script FILE] [--steps "step;step"]
//!              [--chat "line;/command"] (docs/probe.md has every option)
//! aerie-server [...] --replay-setup DIR --captured-registries all|40-60,66|!66
//! aerie-server check [--assets ZIP] [--replay-setup DIR]   (registries the client would refuse)
//! aerie-server proxy --bind 0.0.0.0:5520 --upstream 127.0.0.1:5521 [--capture DIR]
//! ```

mod account;
mod cache;
mod chains;
mod changes;
mod chunk;
mod common;
mod config;
mod connected;
mod crafting;
mod effects;
mod falling;
mod fetch;
mod gather;
mod inventory;
mod items;
mod join;
mod login;
mod perf;
mod probe;
mod processing;
mod proxy;
mod stats;
mod support;
mod terrain;
mod tls;
mod trig;
mod world;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use tracing::{info, warn};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let default_bind = format!("0.0.0.0:{}", aerie_protocol::version::DEFAULT_PORT);
    let bind: SocketAddr = arg(&args, "--bind")
        .unwrap_or(default_bind)
        .parse()
        .context("--bind")?;

    if args.first().map(String::as_str) == Some("proxy") {
        let upstream: SocketAddr = arg(&args, "--upstream")
            .context("proxy needs --upstream")?
            .parse()?;
        let capture = PathBuf::from(arg(&args, "--capture").unwrap_or_else(|| "captures".into()));
        return proxy::run(bind, upstream, capture).await;
    }
    if args.first().map(String::as_str) == Some("probe") {
        let upstream: SocketAddr = arg(&args, "--upstream")
            .context("probe needs --upstream")?
            .parse()?;
        let capture = PathBuf::from(arg(&args, "--capture").unwrap_or_else(|| "probe".into()));
        let opts = probe::Options {
            script: arg(&args, "--script").map(PathBuf::from),
            steps: arg(&args, "--steps"),
            name: arg(&args, "--name"),
            chat: arg(&args, "--chat"),
            send: arg(&args, "--send"),
            goto: arg(&args, "--goto"),
            uuid: arg(&args, "--uuid"),
            look: arg(&args, "--look"),
            use_block: arg(&args, "--use-block"),
            after: match arg(&args, "--sprint").and_then(|s| s.parse().ok()) {
                Some(steps) => Some(probe::sprint_steps(steps, arg(&args, "--after"))?),
                None => arg(&args, "--after"),
            },
            interactions: arg(&args, "--interactions").map(PathBuf::from),
            root_map: arg(&args, "--root-map").map(PathBuf::from),
            listen: arg(&args, "--listen").and_then(|s| s.parse().ok()),
        };
        return probe::run(upstream, capture, opts).await;
    }

    // Before the settings file: fetching the game needs none.
    if args.first().map(String::as_str) == Some("fetch-game") {
        return fetch::run(fetch::Options {
            account: PathBuf::from(
                arg(&args, "--account").unwrap_or_else(|| "aerie-account.json".into()),
            ),
            patchline: arg(&args, "--patchline"),
            out: PathBuf::from(arg(&args, "--out").unwrap_or_else(|| "game".into())),
            list: args.iter().any(|a| a == "--list"),
            keep_package: args.iter().any(|a| a == "--keep-package"),
        })
        .await;
    }

    // The settings file (spec 25), then what the command line overrides.
    let conf = if args.first().map(String::as_str) == Some("check") {
        config::Config::default()
    } else {
        let path = PathBuf::from(arg(&args, "--config").unwrap_or_else(|| "config.json".into()));
        config::Config::load(&path)?.with_overrides(config::Overrides {
            name: arg(&args, "--name"),
            motd: arg(&args, "--motd"),
            password: arg(&args, "--password"),
            max_players: arg(&args, "--max-players")
                .map(|v| v.parse())
                .transpose()
                .context("--max-players")?,
            max_view_radius: arg(&args, "--max-view-radius")
                .map(|v| v.parse())
                .transpose()
                .context("--max-view-radius")?,
            game_mode: arg(&args, "--game-mode"),
        })?
    };
    let use_cache = !args.iter().any(|a| a == "--no-cache");
    let account_file =
        PathBuf::from(arg(&args, "--account").unwrap_or_else(|| "aerie-account.json".into()));
    if args.first().map(String::as_str) == Some("login") {
        return account::login_command(&account_file, arg(&args, "--profile")).await;
    }

    let (server_config, cert_der) = tls::server_config()?;
    let checking = args.first().map(String::as_str) == Some("check");
    let account = if checking {
        None
    } else {
        account::ServerAccount::start(&account_file, &cert_der).await?
    };
    let replay = arg(&args, "--replay-setup")
        .map(|d| login::load_capture(std::path::Path::new(&d)))
        .transpose()?;
    let (mut registries, blocks, block_names, extras) =
        build_registries_cached(arg(&args, "--assets"), replay.as_deref(), use_cache)?;
    let player = join::PlayerParts::new(&registries, &extras);
    if let Some(which) = arg(&args, "--captured-registries") {
        let capture = replay
            .as_deref()
            .context("--captured-registries needs --replay-setup")?;
        use_captured(&mut registries, capture, &which)?;
    }
    let mut refused = check_registries(&registries);
    // WorldSettings lists the common assets; a replay run sends the
    // captured one, so only build it when it is needed or checked.
    let captured_settings = replay
        .as_deref()
        .and_then(|c| c.iter().find(|(i, _)| *i == 20).map(|(_, p)| p.clone()));
    let common_assets = if captured_settings.is_none() || checking {
        let zip = arg(&args, "--assets")
            .map(PathBuf::from)
            .or_else(aerie_assets::pack::Pack::default_path);
        match zip.map(|z| {
            let key = cache::key("assets", &z, None);
            if let Some(l) = use_cache.then(|| cache::load_assets(&key)).flatten() {
                info!("common asset list from the cache ({} assets)", l.len());
                return anyhow::Ok(l);
            }
            let l = common::list(&z)?;
            if use_cache {
                if let Err(e) = cache::save_assets(&key, &l) {
                    warn!("{e:#}");
                }
            }
            Ok(l)
        }) {
            Some(Ok(list)) => Some(list),
            Some(Err(e)) => {
                warn!("common assets not listed: {e:#}");
                None
            }
            None => None,
        }
    } else {
        None
    };
    if checking {
        if let (Some(built), Some(captured)) = (&common_assets, &captured_settings) {
            refused += common::compare(built, captured);
        }
        // The player's stats against the captured player entity.
        if let Some(c) = replay.as_deref() {
            let parts = join::PlayerParts::new(&registries, &extras);
            let stats = |p: &[u8]| -> String {
                let t = aerie_protocol::proto::debug(161, p)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                t.split("EntityStatsUpdate")
                    .nth(1)
                    .unwrap_or("")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            if let (Ok(b), Some((_, cp))) = (
                join::player_entity_packet_for_check(2, &parts),
                c.iter().find(|(i, _)| *i == 161),
            ) {
                let (sb, sc) = (stats(&b), stats(cp));
                if sb == sc {
                    info!("player stats: same as the capture");
                } else {
                    warn!(
                        "player stats differ:\n built    {}\n captured {}",
                        &sb[..sb.len().min(1500)],
                        &sc[..sc.len().min(1500)]
                    );
                }
            }
        }
        // World settings built from the assets against the captured join.
        if let Some(c) = replay.as_deref() {
            for (name, id) in [
                (join::MOVEMENT_SETTINGS, 110u32),
                (join::TIME_SETTINGS, 145),
                (join::WORLD_GAMEPLAY, 185),
            ] {
                let built = extras.iter().find(|(n, _)| n == name).map(|(_, b)| b);
                let captured = c.iter().find(|(i, _)| *i == id).map(|(_, b)| b);
                match (built, captured) {
                    (Some(b), Some(cp)) => {
                        let show = |p: &[u8]| {
                            aerie_protocol::proto::debug(id, p)
                                .ok()
                                .flatten()
                                .unwrap_or_default()
                        };
                        let (tb, tc) = (show(b), show(cp));
                        let diff: Vec<String> = tb
                            .lines()
                            .zip(tc.lines())
                            .filter(|(x, y)| x != y)
                            .map(|(x, y)| format!("built {} / captured {}", x.trim(), y.trim()))
                            .collect();
                        if diff.is_empty() {
                            info!("{name}: same as the capture");
                        } else {
                            warn!("{name}: {} lines differ: {}", diff.len(), diff.join("; "));
                        }
                    }
                    (None, _) => warn!("{name}: not built"),
                    (_, None) => info!("{name}: not in the capture"),
                }
            }
        }
    }
    if checking {
        if registries.is_empty() {
            anyhow::bail!("no registries built (pass --assets)");
        }
        if refused > 0 {
            anyhow::bail!("{refused} problems the client would refuse");
        }
        info!("all {} registries pass the check", registries.len());
        return Ok(());
    }
    let benches = extras
        .iter()
        .find(|(n, _)| n == crafting::BENCHES)
        .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    let book = crafting::Book::new(&registries, &benches);
    let consume = extras
        .iter()
        .find(|(n, _)| n == "consume")
        .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    info!("{} items used up by interactions", consume.lines().count());
    let physics = extras
        .iter()
        .find(|(n, _)| n == support::TABLE)
        .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    let rules = support::Rules::new(&registries, &physics);
    let eye_height = extras
        .iter()
        .find(|(n, _)| n == join::PLAYER_MODEL)
        .and_then(|(_, b)| {
            <aerie_protocol::proto::Model as aerie_protocol::wire::Wire>::decode(b, 0).ok()
        })
        .map_or(1.6, |(m, _)| m.eye_height as f64);
    info!("block support: {} block types", rules.len());
    let (r, i, b) = book.counts();
    info!("crafting: {r} recipes, {i} items, {b} benches");
    // Interactions as the server runs them (spec 18).
    let chain_book = {
        let reg = |id: u32| {
            registries
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, b)| b.as_slice())
        };
        let table = extras
            .iter()
            .find(|(n, _)| n == chains::TABLE)
            .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        match (reg(66), reg(67)) {
            (Some(i), Some(r)) => {
                match chains::Book::build(i, r, reg(68), reg(54), reg(40), &table) {
                    Ok(b) => {
                        info!(
                            "interactions: {} interactions, {} root interactions",
                            b.inters.iter().flatten().count(),
                            b.roots.iter().flatten().count()
                        );
                        b
                    }
                    Err(e) => {
                        warn!("interactions not read: {e}");
                        Default::default()
                    }
                }
            }
            _ => Default::default(),
        }
    };
    let effect_book = {
        let table = extras
            .iter()
            .find(|(n, _)| n == effects::TABLE)
            .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        registries
            .iter()
            .find(|(i, _)| *i == 51)
            .and_then(|(_, b)| effects::Book::build(b, &table).ok())
            .unwrap_or_default()
    };
    let settings = Arc::new(login::Settings {
        name: conf.name.clone(),
        motd: conf.motd.clone(),
        max_players: conf.max_players,
        max_view_radius: conf.max_view_radius,
        password: Some(conf.password.clone()).filter(|p| !p.is_empty()),
        account,
        registries,
        replay,
        hub: {
            let gather = extras
                .iter()
                .find(|(n, _)| n == gather::TABLE)
                .map(|(_, b)| gather::Table::parse(&String::from_utf8_lossy(b)))
                .unwrap_or_default();
            let hub = world::Hub::with_names(blocks, block_names)
                .with_gather(gather)
                .with_perf_hud(args.iter().any(|a| a == "--perf-hud"))
                .with_stats(stats::Types::parse(
                    &extras
                        .iter()
                        .find(|(n, _)| n == stats::TABLE)
                        .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
                        .unwrap_or_default(),
                ))
                .with_creative(conf.creative)
                .with_book(book)
                .with_chains(chain_book)
                .with_effects(effect_book)
                .with_consume(&consume)
                .with_rules(rules)
                .with_eye_height(eye_height);
            // Changed blocks are kept on disk (`--world DIR`, default `aerie-world`).
            let dir = PathBuf::from(arg(&args, "--world").unwrap_or_else(|| "aerie-world".into()));
            if checking { hub } else { hub.with_world(dir)? }
        },
        common_assets,
        built_world: args.iter().any(|a| a == "--built-world"),
        player,
        captured_player: arg(&args, "--captured-player"),
        record: arg(&args, "--record").map(PathBuf::from),
        flat: args.iter().any(|a| a == "--flat"),
        seed: arg(&args, "--seed")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0xae1e),
    });
    let endpoint = quinn::Endpoint::server(server_config, bind)?;
    // The world ticks 30 times a second (spec 10).
    {
        let settings = settings.clone();
        // A thread of its own: the ticks keep their pace whatever the
        // connections' tasks are busy with.
        let runtime = tokio::runtime::Handle::current();
        std::thread::Builder::new()
            .name("world".into())
            .spawn(move || {
                let _runtime = runtime.enter();
                // As the official loop: a tick starts one step after the last
                // one started, never earlier — the thread wakes a little
                // before and the rest is spun, so a tick's time step is the
                // step plus a few microseconds (periodic effects count on it).
                let step = std::time::Duration::from_nanos(1_000_000_000 / 30);
                let mut last = std::time::Instant::now();
                // How long each tick takes (spec 13): the panel each second, a
                // `perf:` line every 10 s.
                let mut perf = perf::Perf::default();
                loop {
                    let next = last + step;
                    // Sleeping wakes late by a millisecond or two on some
                    // systems: the last three are waited out awake.
                    let early = next - std::time::Duration::from_micros(3000);
                    if let Some(wait) = early.checked_duration_since(std::time::Instant::now()) {
                        std::thread::sleep(wait);
                    }
                    while std::time::Instant::now() < next {
                        std::thread::yield_now();
                    }
                    let start = std::time::Instant::now();
                    last = start;
                    settings.hub.tick();
                    let end = std::time::Instant::now();
                    let due = perf.tick(end - start, end, || settings.hub.perf_counts());
                    if let Some(s) = &due.second {
                        settings.hub.perf_panel(s);
                    }
                    if let Some(s) = &due.ten {
                        tracing::info!("{}", s.log_line());
                    }
                }
            })?;
    }
    let mode = if settings.account.is_some() {
        "authenticated"
    } else {
        "insecure: run `aerie-server login` for release clients"
    };
    info!("Aerie listening on {bind} ({mode})");
    let stop = stop_signal();
    tokio::pin!(stop);
    loop {
        let incoming = tokio::select! {
            i = endpoint.accept() => i,
            _ = &mut stop => break,
        };
        let Some(incoming) = incoming else { break };
        let settings = settings.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => {
                    if let Err(e) = login::handle(conn, &settings).await {
                        warn!("connection ended: {e:#}");
                    }
                }
                Err(e) => warn!("handshake failed: {e}"),
            }
        });
    }
    // Ctrl+C or SIGTERM (spec 25): tell every player, then save the world.
    info!("stopping: disconnecting players");
    settings.hub.disconnect_all(&login::stopping_message());
    // The connections' tasks write the packet; give them time, then close.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    settings.hub.save_all();
    endpoint.close(0u32.into(), b"");
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), endpoint.wait_idle()).await;
    info!("stopped");
    Ok(())
}

/// Resolves when the process is asked to stop (Ctrl+C, or SIGTERM on Unix).
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
                return;
            }
            Err(e) => warn!("SIGTERM handler not installed: {e}"),
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// Registry packets, block ids by lower-cased key, block keys as written,
/// other things built from the assets by name.
type Built = cache::Registries;

/// `build_registries`, or its result from the start-up cache.
fn build_registries_cached(
    assets: Option<String>,
    capture: Option<&[(u32, Vec<u8>)]>,
    use_cache: bool,
) -> Result<Built> {
    let zip = assets
        .clone()
        .map(PathBuf::from)
        .or_else(aerie_assets::pack::Pack::default_path);
    let key = zip.as_deref().map(|z| cache::key("registries", z, capture));
    if let (true, Some(k)) = (use_cache, &key) {
        if let Some(r) = cache::load_registries(k) {
            info!(
                "{} registries from the cache (--no-cache rebuilds them)",
                r.0.len()
            );
            return Ok(r);
        }
    }
    let built = build_registries(assets, capture)?;
    if let (true, Some(k)) = (use_cache, &key) {
        if !built.0.is_empty() {
            if let Err(e) = cache::save_registries(k, &built) {
                warn!("{e:#}");
            }
        }
    }
    Ok(built)
}

/// Builds the setup registries from `Assets.zip` (spec 07). With a test
/// capture, its indexes are used, so that built and replayed registries
/// agree. Also returns the block ids by lower-cased key.
fn build_registries(assets: Option<String>, capture: Option<&[(u32, Vec<u8>)]>) -> Result<Built> {
    use aerie_assets::{ctx::Ctx, pack::Pack, registries};
    let Some(path) = assets.map(PathBuf::from).or_else(Pack::default_path) else {
        warn!("no Assets.zip found (pass --assets): sending no registries");
        return Ok(Default::default());
    };
    let t = std::time::Instant::now();
    let ctx = Ctx::new(Pack::open(&path)?);
    if let Some(c) = capture {
        registries::pin_from_capture(&ctx, |id| {
            c.iter().find(|(i, _)| *i == id).map(|(_, b)| b.clone())
        })?;
        // The client refuses a registry with a key twice; compare runs keep
        // the capture's pins as they are.
        ctx.complete_pins();
    }
    let mut out = Vec::new();
    for (id, name, r) in registries::build_all(&ctx) {
        match r {
            Ok(b) => out.push((id, b)),
            Err(e) => warn!("{name} not built: {e}"),
        }
    }
    let blocks = ctx
        .indexes(registries::blocks::PATH, registries::blocks::BUILTIN)
        .map(|m| (*m).clone())
        .unwrap_or_default();
    let block_names = ctx
        .order(registries::blocks::PATH, registries::blocks::BUILTIN)
        .unwrap_or_default();
    // The player's model, for the player's own entity in the built world.
    let mut extras = Vec::new();
    match registries::models::model(&ctx, "Player", None) {
        Ok(Some(m)) => {
            let mut b = Vec::new();
            aerie_protocol::wire::Wire::encode(&m, &mut b)?;
            extras.push((join::PLAYER_MODEL.to_string(), b));
        }
        Ok(None) => warn!("no Player model in the assets"),
        Err(e) => warn!("Player model not built: {e}"),
    }
    // What breaking blocks takes and gives.
    match registries::gathering::table(&ctx) {
        Ok(t) => {
            let count = |c: &str| t.lines().filter(|l| l.starts_with(c)).count();
            info!(
                "gathering: {} blocks, {} tool specs, {} bare-hand specs",
                count("B "),
                count("T "),
                count("U ")
            );
            extras.push((crate::gather::TABLE.to_string(), t.into_bytes()))
        }
        Err(e) => warn!("gathering table not built: {e}"),
    }
    // Items used up by an interaction (eating).
    match registries::consume::table(&ctx) {
        Ok(t) => extras.push(("consume".to_string(), t.into_bytes())),
        Err(e) => warn!("consume table not built: {e}"),
    }
    // Block support: drop types, physics drops, drop lists.
    match registries::physics::table(&ctx) {
        Ok(t) => extras.push((crate::support::TABLE.to_string(), t.into_bytes())),
        Err(e) => warn!("physics table not built: {e}"),
    }
    // What the server needs of entity effects beyond their packets.
    match registries::entity_effects::server_table(&ctx) {
        Ok(t) => extras.push((crate::effects::TABLE.to_string(), t.into_bytes())),
        Err(e) => warn!("effects table not built: {e}"),
    }
    // Interaction types as the server runs them.
    match registries::interactions::server_table(&ctx) {
        Ok(t) => extras.push((crate::chains::TABLE.to_string(), t.into_bytes())),
        Err(e) => warn!("interaction types table not built: {e}"),
    }
    // Stat types as the server runs them.
    match registries::entity_stat_types::table(&ctx) {
        Ok(t) => extras.push((crate::stats::TABLE.to_string(), t.into_bytes())),
        Err(e) => warn!("stats table not built: {e}"),
    }
    // Crafting benches.
    match registries::benches::table(&ctx) {
        Ok(t) => extras.push((crate::crafting::BENCHES.to_string(), t.into_bytes())),
        Err(e) => warn!("bench table not built: {e}"),
    }
    // World settings for the built world, from the gameplay configuration.
    match registries::world_config::config(&ctx) {
        Ok(g) => {
            let mut b = Vec::new();
            aerie_protocol::wire::Wire::encode(
                &registries::world_config::gameplay(&ctx, &g),
                &mut b,
            )?;
            extras.push((join::WORLD_GAMEPLAY.to_string(), b));
            let mut b = Vec::new();
            aerie_protocol::wire::Wire::encode(&registries::world_config::time(&g), &mut b)?;
            extras.push((join::TIME_SETTINGS.to_string(), b));
            match registries::world_config::movement(&ctx, &g, true) {
                Ok(Some(m)) => {
                    let mut b = Vec::new();
                    aerie_protocol::wire::Wire::encode(
                        &aerie_protocol::proto::UpdateMovementSettings {
                            movement_settings: m,
                        },
                        &mut b,
                    )?;
                    extras.push((join::MOVEMENT_SETTINGS.to_string(), b));
                }
                Ok(None) => warn!("the player's movement configuration is not an asset"),
                Err(e) => warn!("movement settings not built: {e}"),
            }
        }
        Err(e) => warn!("gameplay configuration not read: {e}"),
    }
    info!(
        "built {} registries from {} in {:.1?}",
        out.len(),
        path.display(),
        t.elapsed()
    );
    Ok((out, blocks, block_names, extras))
}

/// Test only: sends the captured payload instead of the built one for some
/// registries, to find a registry the client refuses. `which` is `all`, or
/// packet ids and ranges such as `40-60,66`; a leading `!` inverts it.
fn use_captured(
    registries: &mut [(u32, Vec<u8>)],
    capture: &[(u32, Vec<u8>)],
    which: &str,
) -> Result<()> {
    let (invert, list) = match which.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, which),
    };
    let mut ids = Vec::new();
    for part in list.split(',').filter(|p| !p.is_empty()) {
        if part == "all" {
            ids.extend(0..1000);
        } else if let Some((a, b)) = part.split_once('-') {
            ids.extend(a.trim().parse::<u32>()?..=b.trim().parse::<u32>()?);
        } else {
            ids.push(part.trim().parse::<u32>()?);
        }
    }
    let mut swapped = Vec::new();
    for (id, payload) in registries.iter_mut() {
        if ids.contains(id) != invert {
            // The first captured packet with this id (UpdateInteractions is
            // sent twice; the first is the full registry).
            if let Some((_, c)) = capture.iter().find(|(i, _)| i == id) {
                *payload = c.clone();
                swapped.push(*id);
            }
        }
    }
    info!(
        "sending {} captured registries instead of built ones (test only): {:?}",
        swapped.len(),
        swapped
    );
    Ok(())
}

/// Decodes every registry about to be sent and logs what the release client
/// would refuse (duplicate map keys, indexes out of range), before any
/// client sees it. Returns the number of problems.
fn check_registries(registries: &[(u32, Vec<u8>)]) -> usize {
    let mut n = 0;
    for (id, payload) in registries {
        let name = aerie_protocol::table::info(*id).map_or("?", |i| i.name);
        match aerie_protocol::proto::check(*id, payload) {
            Ok(Some(errs)) => {
                for e in &errs {
                    warn!("{name}: the client would refuse this: {e}");
                }
                n += errs.len();
            }
            Ok(None) => {}
            Err(e) => {
                warn!("{name}: does not decode: {e}");
                n += 1;
            }
        }
    }
    n
}
