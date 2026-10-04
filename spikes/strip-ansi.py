#!/usr/bin/env python3
"""Rough text view of `claude logs` output: drop escape sequences, keep printable text."""
import re
import sys

raw = open(sys.argv[1], encoding="utf-8", errors="replace").read()
# cursor moves become line breaks so screen rows don't run together
text = re.sub(r"\x1b\[\d*(;\d*)?[HBf]", "\n", raw)
text = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07|\x1b[()][A-Z0-9]", "", text)
lines = [line.rstrip() for line in text.splitlines() if line.strip()]
print("\n".join(lines))
