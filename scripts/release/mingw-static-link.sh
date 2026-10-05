#!/bin/bash
# Linker wrapper for x86_64-pc-windows-gnu that statically links libstdc++.
# The cc crate emits -lstdc++ (dynamic) for C++ dependencies like DuckDB.
# This wrapper intercepts -lstdc++ and wraps it with -Bstatic/-Bdynamic
# so the final binary doesn't depend on libstdc++-6.dll at runtime.
args=()
for arg in "$@"; do
    if [ "$arg" = "-lstdc++" ]; then
        args+=("-Wl,-Bstatic" "-lstdc++" "-Wl,-Bdynamic")
    else
        args+=("$arg")
    fi
done
exec x86_64-w64-mingw32-g++ "${args[@]}"
