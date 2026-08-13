# Linux build environment for the C-side arithmetic-coder OP TRACE.
#
# WHY THIS EXISTS. `tools/capture_c_trace/build.sh` interposes on the C
# reference's entropy/RD entry points with `-Wl,--wrap=<sym>`. That flag is a
# GNU-ld / lld-ELF extension; Apple's `ld64` rejects it outright, so on macOS
# build.sh falls back to a BYTE-ONLY driver and every symbol-level
# localization (`identity_diff.sh`'s op trace, `tile_map.sh`) is unavailable.
# `docs/WORKING-ON-THIS.md` §5 records that as a standing hole; issue #15's
# root-cause needed it closed.
#
# The container carries ONLY the C toolchain. That is deliberate:
#   * the C reference is the only thing that needs GNU ld;
#   * the port's own trace is produced by `tools/identity_run` on the host,
#     where it is already built and where its bytes are the ones under test;
#   * a Rust toolchain here would add ~1.5 GB of image and a second multi-GB
#     target dir for a build whose output we would then have to prove equal to
#     the host's anyway.
# If you ever DO need the port built in-container, add rustup as a later
# layer — do not fold it into this one (see the layering rule in the global
# CLAUDE.md: cheap-and-stable layers first).
#
# The image is arch-neutral; build it for the SAME architecture as the host
# oracle you are comparing against (`run.sh` does this by default). On an
# Apple-silicon host that means linux/arm64, so C's runtime-dispatched NEON
# kernels are the same ones the macOS oracle picks.
FROM ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    cmake \
    ninja-build \
    git \
    python3 \
    ca-certificates \
    binutils \
    gdb \
    && rm -rf /var/lib/apt/lists/*

# Fail loudly if the one capability this image exists for is missing, rather
# than silently producing the byte-only driver the host already has.
RUN printf 'void __wrap_probe_fn(void){} int probe_fn(void); int main(void){return 0;}\n' > /tmp/p.c \
    && cc -o /tmp/p /tmp/p.c -Wl,--wrap=probe_fn \
    && rm -f /tmp/p /tmp/p.c

WORKDIR /repo
