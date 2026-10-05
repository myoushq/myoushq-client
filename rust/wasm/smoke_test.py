#!/usr/bin/env python3
"""Check the WebAssembly PAKE against python-spake2 in both roles.

Needs python-spake2 (pip install spake2) and Node, after ./build.sh.
"""
import json
import os
import subprocess
import sys
from pathlib import Path

from spake2 import SPAKE2_A, SPAKE2_B

HERE = Path(__file__).resolve().parent
CODE = "4821-K7F3QX"
IDS = dict(idA=b"myous-pair-a", idB=b"myous-pair-b")

NODE = """
const { pakeStart, pakeFinish } = require(%r);
const [op, role, code, seed, peer] = process.argv.slice(1);
const hex = (b) => Buffer.from(b).toString("hex");
const bytes = (h) => Uint8Array.from(Buffer.from(h, "hex"));
try {
  if (op === "start") console.log(hex(pakeStart(role, code, bytes(seed))));
  else console.log(hex(pakeFinish(role, code, bytes(seed), bytes(peer))));
} catch (e) { console.error(String(e)); process.exit(1); }
""" % str(HERE / "pkg" / "myous_pake_wasm.js")


def node(*args):
    return subprocess.run(["node", "-e", NODE, *args], capture_output=True, text=True, check=True).stdout.strip()


def main():
    ok = True
    for py_role, js_role in (("a", "b"), ("b", "a")):
        seed = os.urandom(32).hex()
        py = (SPAKE2_A if py_role == "a" else SPAKE2_B)(CODE.encode(), **IDS)
        py_msg = py.start()
        js_msg = node("start", js_role, CODE, seed)
        same = py.finish(bytes.fromhex(js_msg)).hex() == node("finish", js_role, CODE, seed, py_msg.hex())
        print(f"python side {py_role}, wasm side {js_role}: keys equal = {same}")
        ok &= same

    seed = os.urandom(32).hex()
    py = SPAKE2_A(CODE.encode(), **IDS)
    py_msg = py.start()
    k_py = py.finish(bytes.fromhex(node("start", "b", "4821-AAAAAA", seed))).hex()
    differ = k_py != node("finish", "b", "4821-AAAAAA", seed, py_msg.hex())
    print(f"wrong code: keys differ = {differ}")
    ok &= differ

    bad = subprocess.run(["node", "-e", NODE, "start", "a", CODE, "00"], capture_output=True, text=True)
    print(f"short seed rejected = {bad.returncode != 0} ({bad.stderr.strip()})")
    ok &= bad.returncode != 0
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
