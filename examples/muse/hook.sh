#!/usr/bin/env bash
# Shim: the hook moved into the package in v0.5.0; `myous hook-script`
# prints its path. This keeps hooks registered with the old path working.
PYTHON=${MYOUS_PYTHON:-$HOME/.myous/venv/bin/python}
exec bash "$("$PYTHON" -m myous hook-script)" "$@"
