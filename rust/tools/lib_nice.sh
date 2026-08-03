# Shared low-priority launcher for the gate scripts. Source, don't execute:
#
#     . "$(dirname "$0")/lib_nice.sh"
#     $LOWPRI cargo build ...
#
# WHY. The gates hard-coded `nice -n 19 ionice -c3`. `ionice` is util-linux and
# does not exist on macOS or the BSDs, so every one of those invocations died
# with "nice: ionice: No such file or directory" -- `nice` execs its argument
# vector, so a missing `ionice` takes the whole command down rather than
# degrading. That silently made ten gate scripts unrunnable off Linux
# (screen_ibc_gate, imazen26_*, real_image_matrix, wider_corpus_sweep, ...),
# which is the same class of portability failure as capture_c_trace's
# `-Wl,--wrap` assumption.
#
# Probe for the tool rather than testing `uname`: a Linux container without
# util-linux should degrade the same way, and a BSD that grows an `ionice`
# should benefit. The CPU-priority half (`nice`) is POSIX and always applied --
# it is the part that keeps a long sweep from starving an interactive session.
if command -v ionice >/dev/null 2>&1; then
    LOWPRI="nice -n 19 ionice -c3"
else
    LOWPRI="nice -n 19"
fi
export LOWPRI
