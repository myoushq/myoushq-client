#!/usr/bin/env python3
# Shim: the check moved into the package in v0.5.0 (`myous check`, or
# `python -m myous.muse.check`). This keeps older scheduled tasks working.
from myous.muse.check import main

main()
