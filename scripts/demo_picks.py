#!/usr/bin/env python3
"""Prints a good failed transaction and a good USD-priced swap from the running Sentinel,
so you aren't hunting for them mid-recording. Standard library only.

    python3 scripts/demo_picks.py [base_url] [program_id]
"""
import json
import sys
import urllib.request

base = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8080"
program = sys.argv[2] if len(sys.argv) > 2 else "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4"

rows = json.load(urllib.request.urlopen(f"{base}/api/programs/{program}/transactions?limit=300"))

failed = [r for r in rows if not r["success"] and r.get("instructions")]
swaps = sorted(
    (r for r in rows if r["success"] and r.get("largest_transfer") and (r["largest_transfer"].get("usd") or 0) > 50),
    key=lambda r: -(r["largest_transfer"]["usd"] or 0),
)

if failed:
    print("A failed transaction with a named instruction:\n  ", f"{base}/tx/{failed[0]['signature']}")
else:
    print("No failed transaction with a named instruction yet; give it a minute.")
if swaps:
    t = swaps[0]["largest_transfer"]
    print(f"\nA swap with a USD value ({t['amount']:.2f} {t['symbol']} = ${t['usd']:.2f}):\n  ", f"{base}/tx/{swaps[0]['signature']}")
else:
    print("\nNo USD-priced swap yet; give it a minute.")
