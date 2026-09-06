//! The Leaf deployable. Exists in the manifest from day one, per Issao's decision that Leaf shards
//! become separate processes in a sharded server, so the split is a transport rather than a new
//! binary. The transport is not built yet; until it is, this says so and fails, which is the
//! honest behaviour for a process that cannot serve.

fn main() {
    eprintln!(
        "sim-leaf: the Leaf process transport is not built yet; use LocalLeaf in-process via sim-ingress"
    );
    std::process::exit(2);
}
