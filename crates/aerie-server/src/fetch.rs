//! `aerie-server fetch-game`: gets the game package with the account's own
//! login (spec 26), so a phone (or any machine) needs no other tool. Logs in
//! when there is no saved login, picks a release channel, downloads its
//! package (resuming a dropped download), checks it and unpacks it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use aerie_auth::account::Saved;
use aerie_auth::game::{self, DEFAULT_PATCHLINE};
use aerie_auth::oauth::OAuth;
use anyhow::{Context, Result, bail};

pub struct Options {
    pub account: PathBuf,
    pub patchline: Option<String>,
    pub out: PathBuf,
    /// Only show the channels and their versions.
    pub list: bool,
    /// Keep the downloaded package after unpacking it.
    pub keep_package: bool,
}

/// The file in `out` that says which version is unpacked there.
const VERSION_FILE: &str = "game-version.txt";

pub async fn run(o: Options) -> Result<()> {
    let http = aerie_auth::http();
    if Saved::load(&o.account).await?.is_none() {
        println!("  No saved login yet: logging in first.");
        crate::account::login_command(&o.account, None).await?;
    }
    let mut saved = Saved::load(&o.account)
        .await?
        .context("the login was not saved")?;
    if saved.tokens.expires_soon() {
        saved.tokens = OAuth::server(http.clone()).refresh(&saved.tokens).await?;
        saved.save(&o.account).await?;
    }
    let tokens = &saved.tokens;

    let lines = game::patchlines(&http, tokens).await?;
    if lines.is_empty() {
        bail!("this account cannot download the game (no release channel)");
    }
    if o.list {
        for l in &lines {
            match game::manifest(&http, tokens, &l.name).await {
                Ok(m) => println!("  {:12} {}", l.name, m.version),
                Err(e) => println!("  {:12} (no manifest: {e})", l.name),
            }
        }
        return Ok(());
    }
    let name = match &o.patchline {
        Some(p) if lines.iter().any(|l| &l.name == p) => p.clone(),
        Some(p) => {
            let names: Vec<_> = lines.iter().map(|l| l.name.as_str()).collect();
            bail!("channel {p:?} is not available to this account; it has {names:?}")
        }
        None if lines.iter().any(|l| l.name == DEFAULT_PATCHLINE) => DEFAULT_PATCHLINE.to_owned(),
        None => lines[0].name.clone(),
    };
    let m = game::manifest(&http, tokens, &name).await?;
    println!("  Channel {name}: version {}", m.version);

    tokio::fs::create_dir_all(&o.out).await?;
    let have = tokio::fs::read_to_string(o.out.join(VERSION_FILE))
        .await
        .ok();
    if have.as_deref().map(str::trim) == Some(m.version.as_str()) {
        println!("  Already unpacked in {}.", o.out.display());
        return Ok(());
    }

    let package = o.out.join(format!("package-{}.zip", m.version));
    if !package.exists() {
        let dl = game::download_client();
        let start = Instant::now();
        let mut shown = u64::MAX;
        // Bytes a resumed download already had, left out of the rate.
        let mut base = None;
        let mut progress = |done: u64, total: Option<u64>| {
            let base = *base.get_or_insert(done);
            // A line per percent (per 64 MB when the size is unknown).
            let step = match total {
                Some(t) if t > 0 => done * 100 / t,
                _ => done >> 26,
            };
            if step != shown {
                shown = step;
                let mb = done as f64 / 1e6;
                let rate = (done - base) as f64 / 1e6 / start.elapsed().as_secs_f64().max(0.001);
                match total {
                    Some(t) => println!(
                        "  {step:3}%  {mb:.0} of {:.0} MB  {rate:.1} MB/s",
                        t as f64 / 1e6
                    ),
                    None => println!("  {mb:.0} MB  {rate:.1} MB/s"),
                }
            }
        };
        game::download(&dl, tokens, &m, &package, &mut progress).await?;
        println!("  Downloaded and checked.");
    }

    println!("  Unpacking into {}...", o.out.display());
    let (zip, out) = (package.clone(), o.out.clone());
    tokio::task::spawn_blocking(move || unpack(&zip, &out)).await??;
    tokio::fs::write(o.out.join(VERSION_FILE), &m.version).await?;
    if !o.keep_package {
        tokio::fs::remove_file(&package).await?;
    }
    for f in find(&o.out, &["Assets.zip"]) {
        println!("  Assets: {}", f.display());
    }
    for f in find(&o.out, &[".jar"]) {
        println!("  Server jar: {}", f.display());
    }
    Ok(())
}

/// Unpacks a zip into `out`, refusing entries that would land outside it.
fn unpack(zip: &Path, out: &Path) -> Result<()> {
    let mut a = zip::ZipArchive::new(std::fs::File::open(zip)?)?;
    for i in 0..a.len() {
        let mut e = a.by_index(i)?;
        let Some(rel) = e.enclosed_name() else {
            bail!("the package has an unsafe path: {}", e.name());
        };
        let dest = out.join(rel);
        if e.is_dir() {
            std::fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut f = std::fs::File::create(&dest)?;
        std::io::copy(&mut e, &mut f)?;
        #[cfg(unix)]
        if let Some(mode) = e.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode & 0o755))?;
        }
    }
    Ok(())
}

/// Files under `dir` (two levels deep) whose name ends with one of `ends`.
fn find(dir: &Path, ends: &[&str]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut walk = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = walk.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() && depth < 2 {
                walk.push((p, depth + 1));
            } else if p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| ends.iter().any(|x| n.ends_with(x)))
            {
                found.push(p);
            }
        }
    }
    found.sort();
    found
}
