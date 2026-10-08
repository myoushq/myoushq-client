#!/usr/bin/env python3
# Shim: the watcher moved into the package in v0.5.0 (`myous watcher`, or
# `python -m myous.muse.watcher`). This keeps older hooks and jobs working.
from myous.muse.watcher import main

main()
