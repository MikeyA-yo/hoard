//! `hoard docker`: Docker's disk usage and the commands that free it.
//! Read-only; Hoard never runs the commands it shows.

use anyhow::Result;
use hoard_core::docker::{self, Engine};
use hoard_core::format;
use hoard_core::rules::Safety;

pub fn run(json: bool) -> Result<()> {
    let report = docker::report();
    if json {
        serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
        println!();
        return Ok(());
    }

    match &report.engine {
        Engine::NotInstalled => println!("Docker: not installed (no docker command found)."),
        Engine::TimedOut { seconds } => println!(
            "Docker: did not answer within {seconds} seconds. It may be starting or busy; try again in a moment."
        ),
        Engine::Unavailable { reason } => {
            println!("Docker: not answering. Start Docker Desktop to see what is inside it.");
            println!("    {reason}");
        }
        Engine::Running { categories } => {
            let reclaimable: u64 = categories.iter().map(|c| c.reclaimable).sum();
            let total: u64 = categories.iter().map(|c| c.size).sum();
            println!(
                "Docker: {} used, about {} not in use. Hoard runs none of these commands; copy the ones you want.",
                format::bytes(total),
                format::bytes(reclaimable)
            );
            for c in categories {
                let label = match c.safety {
                    Safety::Safe => "SAFE",
                    Safety::Review => "REVIEW",
                    Safety::Caution => "CAUTION",
                };
                println!();
                println!(
                    "[{label}] {}  {}  ({} of {} in use), {} not in use",
                    c.name,
                    format::bytes(c.size),
                    format::count(c.active),
                    format::count(c.total),
                    format::bytes(c.reclaimable),
                );
                println!("    {}", c.explain);
                println!("    $ {}", c.command);
            }
            println!();
            println!("Sizes use 1 GB = 1024 MB; docker system df uses 1000, so its numbers look about 7% larger.");
            println!();
            println!("All but volumes at once:");
            println!("    $ {}", report.prune_all);
        }
    }

    for img in &report.disk_images {
        println!();
        println!(
            "Disk image: {}  {} on disk",
            img.path.display(),
            format::bytes(img.allocated)
        );
        match &img.compact {
            None => println!("    Space freed inside Docker goes back to the drive by itself."),
            Some(steps) => {
                println!(
                    "    It does not shrink by itself: space freed inside Docker stays reserved until it is compacted."
                );
                for (i, s) in steps.iter().enumerate() {
                    println!("    {}. {}", i + 1, s.text);
                    if let Some(cmd) = &s.command {
                        for line in cmd.lines() {
                            println!("         {line}");
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
