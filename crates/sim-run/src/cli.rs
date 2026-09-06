//! The `sim-run` command line. Subcommands dispatch into the report, arena and ingress crates; this
//! file owns argument parsing and nothing else.

use sim_report::{apply_override, check_comparable, emit, run_all, split_args, Opts, DEFAULT_TELEMETRY_BUDGET_BYTES};
use sim_scenario::Scenario;
use sim_leaf as sim;

pub fn cli(args: Vec<String>) -> Result<(), String> {
    let mut args = args.into_iter();
    let cmd = args.next().unwrap_or_else(|| "help".into());
    let rest: Vec<String> = args.collect();
    match cmd.as_str() {
        "run" => {
            if rest.is_empty() {
                return Err("usage: sim-run run <scenario.txt> [--out FILE] [--set k=v ...]".into());
            }
            let (paths, o) = split_args(&rest)?;
            let runs = run_all(&paths, &o.overrides)?;
            emit(&runs, o.out.as_deref().unwrap_or("out/report.html"), &o)
        }
        "compare" => {
            let (paths, o) = split_args(&rest)?;
            if paths.len() < 2 {
                return Err("compare needs at least two scenarios".into());
            }
            let runs = run_all(&paths, &o.overrides)?;
            check_comparable(&runs)?;
            emit(&runs, o.out.as_deref().unwrap_or("out/compare.html"), &o)
        }
        "sweep" => {
            // One scenario, one key, several values. The cheapest way to see a trend.
            if rest.len() < 2 {
                return Err("usage: sim-run sweep <scenario.txt> --over key=v1,v2,v3".into());
            }
            let base = &rest[0];
            let mut key = String::new();
            let mut values: Vec<String> = Vec::new();
            let mut out = None;
            let mut telemetry = None;
            let mut budget_mb = DEFAULT_TELEMETRY_BUDGET_BYTES / (1024 * 1024);
            let mut i = 1;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--telemetry" => {
                        telemetry = rest.get(i + 1).cloned();
                        i += 2;
                    }
                    "--telemetry-budget-mb" => {
                        budget_mb = rest
                            .get(i + 1)
                            .and_then(|v| v.parse().ok())
                            .ok_or("--telemetry-budget-mb needs a number")?;
                        i += 2;
                    }
                    "--over" => {
                        let spec = rest.get(i + 1).ok_or("--over needs key=v1,v2")?;
                        let (k, vs) = spec.split_once('=').ok_or("--over needs key=v1,v2")?;
                        key = k.to_string();
                        values = vs.split(',').map(|s| s.to_string()).collect();
                        i += 2;
                    }
                    "--out" => {
                        out = rest.get(i + 1).cloned();
                        i += 2;
                    }
                    other => return Err(format!("unexpected argument {other:?}")),
                }
            }
            let text = std::fs::read_to_string(base).map_err(|e| format!("{base}: {e}"))?;
            let mut runs = Vec::new();
            for v in &values {
                let mut sc = Scenario::parse(&text)?;
                apply_override(&mut sc, &key, v)?;
                sc.name = format!("{} = {}", key, v);
                runs.push(sim::run(&sc)?);
            }
            let o = Opts { out: out.clone(), telemetry, budget_mb, overrides: Vec::new() };
            emit(&runs, o.out.as_deref().unwrap_or("out/sweep.html"), &o)
        }
        // Wired here, as the arena's own doc comment asked: `sim-run arena [--cap C] [--policies a,b]
        // [--capacity-sweep R1,R2] [scenario.txt ...]`, with no scenarios meaning the held-out suite.
        "arena" => sim_arena::arena_main(rest),
        "serve" => {
            // Serving is not simulation, so threads are fine here; the no-threads rule in CLAUDE.md
            // is about keeping the simulated clock the only notion of time.
            let mut dir = "web/dist".to_string();
            let port: u16 = std::env::var("PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8080);
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--dir" => {
                        dir = rest.get(i + 1).cloned().ok_or("--dir needs a path")?;
                        i += 2;
                    }
                    other => return Err(format!("unexpected argument {other:?}")),
                }
            }
            sim_ingress::serve(&dir, port)
        }
        _ => {
            println!("sim-run <command>");
            println!("  serve   [--dir DIR]               static server; PORT from the environment");
            println!("  run     <scenario.txt> [...]      one or more scenarios into one report");
            println!("  compare <a.txt> <b.txt> [...]     same load, different policies, checked");
            println!("  sweep   <s.txt> --over key=v1,v2  one parameter across several values");
            println!("  arena   [scenario.txt ...]        score every policy on a slate; default is the held-out suite");
            println!("  options: --out FILE, --set key=value");
            println!("           --telemetry DIR            write analysable CSV telemetry there");
            println!("           --telemetry-budget-mb N    cap it, default 100; over budget it is");
            println!("                                      stratified, and manifest.csv says how");
            Ok(())
        }
    }
}

