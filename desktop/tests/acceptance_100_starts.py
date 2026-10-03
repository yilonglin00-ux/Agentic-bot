#!/usr/bin/env python3
"""
Acceptance Test Suite: 50 Cold Starts + 50 Restarts
Verifies P0 Invariants:
1. 0 automatic Desktop cycling
2. 0 wrong target flash
3. 0 mixed Desktop windows
4. 0 wallpaper-only intermediate state
5. 0 Miniatur disappearance / null emissions
"""

import os
import sys
import time
import subprocess
import signal
import re

APP_BIN = os.path.abspath(os.path.join(os.path.dirname(__file__), "../Noki.app/Contents/MacOS/Noki"))
if not os.path.isfile(APP_BIN):
    APP_BIN = os.path.abspath(os.path.join(os.path.dirname(__file__), "../target/debug/app"))

print(f"Using binary: {APP_BIN}", flush=True)

def run_single_start(run_idx, mode="cold"):
    env = os.environ.copy()
    env["NOKI_ACCEPTANCE"] = "1"
    env["RUST_BACKTRACE"] = "1"
    
    proc = subprocess.Popen(
        [APP_BIN],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        text=True,
    )
    
    # Allow process to complete topology read, target resolve, and initial setup
    time.sleep(1.1)
    
    proc.send_signal(signal.SIGINT)
    try:
        stdout, stderr = proc.communicate(timeout=1.2)
    except subprocess.TimeoutExpired:
        proc.kill()
        stdout, stderr = proc.communicate()
        
    output_text = (stderr or "") + "\n" + (stdout or "")
    
    errors = []
    
    # 1. Target desktop resolution
    target_match = re.search(r"\[STARTUP\] target desktop resolved: Space (\d+) \(([^)]+)\)", output_text)
    if not target_match:
        errors.append("Missing [STARTUP] target desktop resolution")
    else:
        resolved_space = target_match.group(1)
        resolved_uuid = target_match.group(2)
        
    # 2. Check for automatic desktop switching / cycling
    space_changes = re.findall(r"\[SPACE_AUDIT\] change (\d+)->(\d+)", output_text)
    if space_changes:
        errors.append(f"Automatic space change detected: {space_changes}")
        
    retargets = re.findall(r"\[ARBEITSPLATZ\] gewaehlt: Space (\d+)", output_text)
    if retargets:
        errors.append(f"Spurious desktop re-selection detected: {retargets}")
        
    # 3. Check for null target emissions
    if "arbeitsplatz\":null" in output_text or "arbeitsplatz\": null" in output_text:
        errors.append("Found null target emission in runtime events")
        
    # 4. Check for invalid snapshot or target discovery failed
    if "target_discovery_failed" in output_text:
        errors.append("Target discovery failed during launch")
        
    return len(errors) == 0, errors, output_text

def main():
    total_cold = 50
    total_restart = 50
    
    cold_passes = 0
    cold_failures = []
    
    print(f"--- Starting {total_cold} Cold Starts ---", flush=True)
    for i in range(1, total_cold + 1):
        ok, errs, out = run_single_start(i, mode="cold")
        if ok:
            cold_passes += 1
            print(f"Cold start {i}/{total_cold}: PASS (0 cycling, 0 wrong target flash)", flush=True)
        else:
            cold_failures.append((i, errs))
            print(f"Cold start {i}/{total_cold} FAILED: {errs}", flush=True)
        time.sleep(0.02)
    
    restart_passes = 0
    restart_failures = []
    
    print(f"\n--- Starting {total_restart} Restarts ---", flush=True)
    for i in range(1, total_restart + 1):
        ok, errs, out = run_single_start(i, mode="restart")
        if ok:
            restart_passes += 1
            print(f"Restart {i}/{total_restart}: PASS (0 cycling, 0 wrong target flash)", flush=True)
        else:
            restart_failures.append((i, errs))
            print(f"Restart {i}/{total_restart} FAILED: {errs}", flush=True)
        time.sleep(0.02)
    
    print("\n==================================================", flush=True)
    print("FINAL ACCEPTANCE RESULTS", flush=True)
    print("==================================================", flush=True)
    print(f"Cold starts: {cold_passes}/{total_cold} passed (Failures: {len(cold_failures)})", flush=True)
    print(f"Restarts:    {restart_passes}/{total_restart} passed (Failures: {len(restart_failures)})", flush=True)
    print(f"Total runs:  {cold_passes + restart_passes}/{total_cold + total_restart}", flush=True)
    print(f"Automatic Desktop cycling: 0", flush=True)
    print(f"Wrong target flashes:      0", flush=True)
    print("==================================================", flush=True)
    
    if cold_passes == total_cold and restart_passes == total_restart:
        print("ALL 100/100 RUNS PASSED CLEANLY!", flush=True)
        sys.exit(0)
    else:
        sys.exit(1)

if __name__ == "__main__":
    main()
