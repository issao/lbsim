# Build toolchain setup (lbsim sandbox)

Status: **working**. `cargo build`, `cargo build --release`, and `cargo test` all
succeed for the native `x86_64-unknown-linux-gnu` target, including proc-macro
crates, threads, C build scripts (`cc` crate), C++ (`g++`), and protobuf codegen.

## The problem

`cargo build` failed with `error: linker \`cc\` not found`.

Diagnosis: this Debian 13 (trixie) image *already has* everything needed to link
a Rust binary — `libc6-dev` (`crt1.o`, `crti.o`, `crtn.o`, `Scrt1.o`, `libc.so`,
`libc.a`), `libgcc-14-dev` (`libgcc.a`, `libgcc_eh.a`, `crtbeginS.o`,
`crtendS.o`), and `binutils` 2.44 (`/usr/bin/ld`, `as`, `ar`). The *only* missing
piece was the `gcc` driver binary itself (package `gcc-14-x86-64-linux-gnu`),
which rustc invokes as `cc`.

We are uid 1000 with no `sudo` binary, so packages cannot be installed normally.
The fix is to `apt-get download` the gcc packages (works unprivileged) and
`dpkg -x` them into a user prefix. GCC computes its internal search paths
relative to its own executable (`make_relative_prefix`), so a relocated Debian
gcc works out of the box.

Approaches 1 and 2 from the original plan (bundled `rust-lld` + musl
self-contained) were **not needed** — the native gnu target links fine. This is
better than a musl-only setup: normal dynamic linking, no cross-compilation,
proc macros build as host dylibs with no special casing.

## What is installed and where

Everything is under `$HOME`. Nothing was installed system-wide.

| Path | Contents |
| --- | --- |
| `/home/agents/local/usr/bin/` | `gcc`, `cc` (symlink to gcc), `g++`, `c++`, `cpp`, `gcc-ar`, `gcc-nm`, `gcc-ranlib`, `x86_64-linux-gnu-gcc`, `x86_64-linux-gnu-g++` |
| `/home/agents/local/usr/libexec/gcc/x86_64-linux-gnu/14/` | `cc1`, `cc1plus`, `collect2`, `lto1`, `lto-wrapper`, `liblto_plugin.so` |
| `/home/agents/local/usr/lib/gcc/x86_64-linux-gnu/14/` | `libgcc.a`, `libgcc_eh.a`, `crtbegin*.o`, `crtend*.o`, `libstdc++.a/.so`, headers |
| `/home/agents/local/usr/include/`, `/home/agents/local/usr/lib/x86_64-linux-gnu/` | `libc6-dev` / `linux-libc-dev` / `libcrypt-dev` / `libstdc++-14-dev` copies |
| `/home/agents/local/bin/protoc` | statically linked `protoc` (libprotoc 31.1) |
| `/home/agents/.local/bin/` | symlinks: `cc`, `gcc`, `g++`, `c++`, `cpp`, `gcc-ar`, `gcc-nm`, `gcc-ranlib`, `x86_64-linux-gnu-gcc`, `x86_64-linux-gnu-g++`, `protoc` |
| `/home/agents/.cargo/config.toml` | explicit linker/CC/CXX/PROTOC (PATH-independent fallback) |

`/home/agents/.local/bin` is already prepended to `PATH` by both
`~/.profile` (line 25) and `~/.bashrc` (line 114), so in any login or
interactive shell `cc` and `protoc` are simply on `PATH` and **no environment
variables or per-project config are required**.

Versions: `gcc (Debian 14.2.0-19) 14.2.0`, `GNU ld 2.44`, `libprotoc 31.1`,
`cargo`/`rustc` 1.98.0.

## Reproducing the install from scratch

```bash
set -e
WORK=$(mktemp -d)
cd "$WORK"

# 1. Download the gcc/g++ driver + backend packages (no root needed).
#    deb.debian.org is reachable from this sandbox.
TMPDIR="$WORK" apt-get download \
  gcc gcc-14 gcc-x86-64-linux-gnu gcc-14-x86-64-linux-gnu \
  cpp cpp-14 cpp-x86-64-linux-gnu cpp-14-x86-64-linux-gnu \
  g++ g++-14 g++-x86-64-linux-gnu g++-14-x86-64-linux-gnu \
  libstdc++-14-dev

# 2. Extract into the user prefix.
mkdir -p "$HOME/local"
for f in *.deb; do dpkg -x "$f" "$HOME/local"; done

# 3. Also mirror the already-installed dev packages into the prefix so the
#    relocated gcc finds headers/CRT objects under its own relative paths.
#    (These .debs are still in the apt cache; otherwise apt-get download them.)
for p in libgcc-14-dev libc6-dev libc-dev-bin linux-libc-dev libcrypt-dev; do
  deb=$(ls /var/cache/apt/archives/${p}_*.deb 2>/dev/null | head -1)
  [ -n "$deb" ] || { TMPDIR="$WORK" apt-get download "$p"; deb=$(ls ${p}_*.deb | head -1); }
  dpkg -x "$deb" "$HOME/local"
done

# 4. Provide the `cc` / `c++` driver names.
ln -sf gcc "$HOME/local/usr/bin/cc"
ln -sf g++ "$HOME/local/usr/bin/c++"

# 5. Put the drivers on PATH (~/.local/bin is already on PATH via ~/.profile).
mkdir -p "$HOME/.local/bin"
for t in cc gcc g++ c++ cpp gcc-ar gcc-nm gcc-ranlib \
         x86_64-linux-gnu-gcc x86_64-linux-gnu-g++; do
  ln -sf "$HOME/local/usr/bin/$t" "$HOME/.local/bin/$t"
done

# 6. protoc: vendored by the protoc-bin-vendored crate (statically linked).
#    Build any crate that depends on it once, then copy the binary out:
mkdir -p "$HOME/local/bin"
cp "$(find "$HOME/.cargo/registry/src" -path '*protoc-bin-vendored-linux-x86_64*/bin/protoc' | head -1)" \
   "$HOME/local/bin/protoc"
chmod +x "$HOME/local/bin/protoc"
ln -sf "$HOME/local/bin/protoc" "$HOME/.local/bin/protoc"

rm -rf "$WORK"
```

## `~/.cargo/config.toml`

This file is **not** required when `~/.local/bin` is on `PATH`, but it makes
builds work from a bare non-login `sh` (whose `PATH` is
`/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`) and from any
tool that spawns cargo with a minimal environment. It lives at
`/home/agents/.cargo/config.toml` — deliberately user-global, **not** inside the
repo, so the repo stays portable.

```toml
# Toolchain shim for this sandbox: no system gcc/cc is installed.
# A relocated Debian gcc-14 lives under /home/agents/local/usr.

[target.x86_64-unknown-linux-gnu]
linker = "/home/agents/local/usr/bin/gcc"

[env]
CC = "/home/agents/local/usr/bin/gcc"
CXX = "/home/agents/local/usr/bin/g++"
PROTOC = "/home/agents/local/bin/protoc"
```

`[env]` entries do not use `force = true`, so an explicitly exported `CC`,
`CXX`, or `PROTOC` still wins.

### Equivalent as environment variables

If you prefer not to rely on the cargo config:

```bash
export PATH="$HOME/.local/bin:$PATH"
export CC=/home/agents/local/usr/bin/gcc
export CXX=/home/agents/local/usr/bin/g++
export PROTOC=/home/agents/local/bin/protoc
```

## Nothing is needed inside the repo

`lbsim` needs **no** `.cargo/config.toml`, no `build.rs` linker hacks, and no
`--target` flag. Plain `cargo build` / `cargo test` work. Do not commit a
sandbox-specific `.cargo/config.toml` into the repo.

## Verified

All checks below were actually run and passed.

- **(a)** A lib + bin crate depending on `serde` (with `derive`), `serde_json`
  and `rayon` builds with `cargo build --release`.
- **(b)** `cargo test` runs; the lib unit test passes (`1 passed; 0 failed`),
  plus bin and doc test harnesses link and run.
- **(c)** `./target/release/probe` executes and prints `OK hello 42 sum=55`.
  It runs under `env -i` (empty environment), and `ldd` shows normal dynamic
  linking against `libgcc_s.so.1`, `libc.so.6`, `ld-linux-x86-64.so.2`.
- **(d)** Proc macros work: `serde_derive` and `syn` compile and link as host
  dylibs (this is the separate `-Cprefer-dynamic` proc-macro link path).
- Extra: the `cc` crate compiles a C file in a `build.rs` and the resulting
  FFI call works. `g++` compiles and links a C++ program.
- Extra: `tonic-prost-build` 0.14 + `prost` 0.14 compile a `.proto` and the
  generated code builds and runs (`proto ok: hi 5`).
- All of the above also pass with `PATH` stripped to
  `/home/agents/.cargo/bin:/usr/local/bin:/usr/bin:/bin`, i.e. relying only on
  `~/.cargo/config.toml`.

## protobuf / protoc

`protoc` **is** available, three independent ways:

1. `protoc` is on `PATH` (`~/.local/bin/protoc` -> `~/local/bin/protoc`,
   statically linked, `libprotoc 31.1`), and `PROTOC` points at it in
   `~/.cargo/config.toml`. This is the recommended route — nothing extra in
   `Cargo.toml`.
2. The `protoc-bin-vendored = "3"` crate works as a build-dependency with no
   system protoc; it ships a prebuilt static Linux x86_64 binary. Use it if you
   want the repo to be self-contained on other machines:
   ```rust
   // build.rs
   fn main() {
       std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path().unwrap());
       tonic_prost_build::compile_protos("proto/hello.proto").unwrap();
   }
   ```
3. Downloading a release tarball also works — deb.debian.org, crates.io and
   general HTTP egress are all reachable.

Note on crate names for the tonic 0.14 line: codegen moved out of `tonic-build`.
You need **three** crates, and forgetting `tonic-prost` yields a confusing
`cannot find module or crate 'tonic_prost'` error in the generated file:

```toml
[dependencies]
prost = "0.14"
tonic = "0.14"
tonic-prost = "0.14"   # required at runtime by generated code

[build-dependencies]
tonic-prost-build = "0.14"
```

## Caveats

- **The install lives in `$HOME`, not in an image layer.** If `/home/agents` is
  wiped or the container is rebuilt from the base image, re-run the install
  script above. It takes a few seconds (~47 MB of .debs).
- **Not a hermetic toolchain.** The relocated gcc still resolves some library
  paths to the system `/usr/lib/x86_64-linux-gnu` (see `LIBRARY_PATH` in
  `gcc -v`). That is fine here because `libc6-dev` and `libgcc-14-dev` are
  genuinely installed system-wide; it just means the prefix is not portable to a
  different base image.
- **No `sudo`/root**, so `apt-get install` will never work; only
  `apt-get download` + `dpkg -x`. Do not add build steps that assume root.
- **Version pinning**: the downloaded gcc is 14.2.0-19 and matches the
  system-installed `libgcc-14-dev` / `libstdc++-14-dev` exactly. If you ever
  re-download, keep the gcc major version at **14** so it matches the installed
  `/usr/lib/gcc/x86_64-linux-gnu/14` tree.
- **musl is not used.** `x86_64-unknown-linux-musl` and `wasm32-unknown-unknown`
  targets are installed and the musl target additionally has rustup's
  `self-contained` CRT objects + `rust-lld`, so a static musl build is available
  as a fallback (`cargo build --target x86_64-unknown-linux-musl`) — but the gnu
  target is the supported path and needs no flags.
- **Build speed is fine** — no linker slowness observed; the release build of
  the serde + rayon probe finished in under 3 seconds.
