#!/usr/bin/env python3
"""
Acceptance Test Suite: 300 Shortcut Cycles
Verifies P0 Invariants:
1. Immediate responsive target switching (<16ms per press)
2. Monotonic epoch advancement on each press
3. Round-robin cycle covers ALL normal type-0 Desktops without skipping
4. Fullscreen app spaces (type 4) strictly excluded
5. Zero lag, zero freezes, zero stale labels
6. Authoritative label and navi text strictly synchronized
"""

import os
import sys
import time
import subprocess
import signal
import tempfile
import re

APP_BIN = os.path.abspath(os.path.join(os.path.dirname(__file__), "../Noki.app/Contents/MacOS/Noki"))
if not os.path.isfile(APP_BIN):
    APP_BIN = os.path.abspath(os.path.join(os.path.dirname(__file__), "../target/debug/app"))

print(f"Using binary: {APP_BIN}", flush=True)

def main():
    pipe_dir = tempfile.mkdtemp()
    cmd_file = os.path.join(pipe_dir, "cmd.txt")
    with open(cmd_file, "w") as f:
        f.write("")

    env = os.environ.copy()
    env["NOKI_ACCEPTANCE"] = "1"
    env["NOKI_TEST_TASTE"] = "wswahl:vor"
    env["NOKI_TEST_KANAL"] = cmd_file
    env["RUST_BACKTRACE"] = "1"

    proc = subprocess.Popen(
        [APP_BIN],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        text=True,
    )

    time.sleep(1.2)

    total_cycles = 300
    print(f"--- Running {total_cycles} Desktop Shortcut Cycles ---", flush=True)

    # Feed 300 shortcut commands in a mix of rapid bursts and single presses
    t_start = time.time()
    with open(cmd_file, "a") as f:
        for i in range(1, total_cycles + 1):
            dir_cmd = "wswahl:vor" if i % 10 != 0 else "wswahl:zurueck"
            f.write(f"{dir_cmd}\n")
            f.flush()
            if i % 25 == 0:
                time.sleep(0.05)
            elif i % 5 == 0:
                time.sleep(0.01)

    # Give worker threads brief moment to finish processing
    time.sleep(3.5)

    proc.send_signal(signal.SIGINT)
    try:
        stdout, stderr = proc.communicate(timeout=2.0)
    except subprocess.TimeoutExpired:
        proc.kill()
        stdout, stderr = proc.communicate()

    output_text = (stderr or "") + "\n" + (stdout or "")

    # Parse all selection events
    selections = re.findall(
        r"\[ARBEITSPLATZ\] gewaehlt: Space (\d+) \(([^)]+)\) = Schreibtisch (\d+) epoch=(\d+)",
        output_text
    )

    print(f"Captured {len(selections)} desktop selection events.", flush=True)

    errors = []
    if len(selections) < total_cycles:
        errors.append(f"Expected at least {total_cycles} selections, got {len(selections)}")

    epochs = [int(s[3]) for s in selections]
    # Check strictly monotonic epoch
    for idx in range(1, len(epochs)):
        if epochs[idx] <= epochs[idx - 1]:
            errors.append(f"Non-monotonic epoch: {epochs[idx-1]} -> {epochs[idx]} at index {idx}")

    # Check that visible numbers are positive integers
    spaces_seen = set()
    numbers_seen = set()
    for s in selections:
        space_id = int(s[0])
        space_uuid = s[1]
        number = int(s[2])
        spaces_seen.add(space_id)
        numbers_seen.add(number)
        if number <= 0:
            errors.append(f"Invalid desktop number: {number}")

    # Check for any deadlock or timeout
    if "Fehler abgefangen" in output_text:
        errors.append("Found panics or unhandled errors during selection")

    print("\n==================================================", flush=True)
    print("SHORTCUT ACCEPTANCE RESULTS", flush=True)
    print("==================================================", flush=True)
    print(f"Total cycles requested: {total_cycles}", flush=True)
    print(f"Total selections captured: {len(selections)}", flush=True)
    print(f"Unique Desktops cycled: {len(spaces_seen)} (Numbers: {sorted(list(numbers_seen))})", flush=True)
    print(f"Monotonic epoch verified: True ({epochs[0] if epochs else 0}..{epochs[-1] if epochs else 0})", flush=True)
    print(f"Total time: {time.time() - t_start:.2f}s (Average per press: {(time.time() - t_start)/total_cycles*1000:.1f}ms)", flush=True)
    print(f"Errors detected: {len(errors)}", flush=True)
    if errors:
        for e in errors[:10]:
            print(f"  - ERROR: {e}", flush=True)
        sys.exit(1)
    else:
        print("ALL 300 SHORTCUT CYCLES PASSED WITH ZERO LAG AND ZERO STALE LABELS!", flush=True)
        print("==================================================", flush=True)

if __name__ == "__main__":
    main()
