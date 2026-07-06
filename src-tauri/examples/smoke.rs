//! Standalone integration check for the usage endpoint.
//! `cargo run --bin smoke` — reads the local token, fetches usage, prints a
//! parsed summary. Never prints the token itself.

use claude_usage_lib::{ensure_token, usage};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = usage::build_client()?;
    let token = ensure_token(&client, false).await?;

    let usage = match usage::fetch_usage(&client, &token).await? {
        Some(u) => u,
        None => {
            eprintln!("401 Unauthorized — token rejected. Run Claude Code to refresh, or enable self-refresh.");
            std::process::exit(1);
        }
    };

    println!(
        "five_hour : {:>5.1}%  resets_at={:?}",
        usage.five_hour.utilization, usage.five_hour.resets_at
    );
    println!(
        "seven_day : {:>5.1}%  resets_at={:?}",
        usage.seven_day.utilization, usage.seven_day.resets_at
    );
    println!("limits ({}):", usage.limits.len());
    for l in &usage.limits {
        println!(
            "  kind={:<14} scope={:<10} {:>5.1}%  severity={:<8} active={} resets_at={:?}",
            l.kind,
            usage::Scope::key(&l.scope),
            l.percent,
            l.severity.clone().unwrap_or_else(|| "-".into()),
            l.is_active,
            l.resets_at
        );
    }
    Ok(())
}
