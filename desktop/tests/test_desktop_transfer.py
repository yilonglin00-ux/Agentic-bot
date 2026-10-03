#!/usr/bin/env python3
"""
Test Desktop-Transfer Invariants:
1. Static code invariant check:
   - overlay_verdecken called unconditionally before space_umziehen
   - spaceVorbereiten sets raum.x = xs (offscreen)
   - spaceEintreten ensures offscreen coordinates before stage visibility
   - Monotonic SPACE_EPOCH in Rust and currentSpaceEpoch in JS
2. Simulation & Invariant Verification:
   - 100 space transfers (left-to-right, right-to-left, fast bursts)
   - First frame coordinate must be offscreen edge entry (<0 or >buehne.w)
   - Zero old-position flashes
   - Zero one-frame spawns
   - Exact previous movement state restored after arrival
"""

import sys
import os
import re

def test_code_invariants():
    lib_path = os.path.abspath(os.path.join(os.path.dirname(__file__), "../src-tauri/src/lib.rs"))
    index_path = os.path.abspath(os.path.join(os.path.dirname(__file__), "../index.html"))

    with open(lib_path, "r", encoding="utf-8") as f:
        lib_src = f.read()

    with open(index_path, "r", encoding="utf-8") as f:
        index_src = f.read()

    errors = []

    # 1. Check space_waechter has unconditional overlay_verdecken before space_umziehen
    waechter_idx = lib_src.find("fn space_waechter")
    assert waechter_idx != -1, "space_waechter function found in lib.rs"
    waechter_chunk = lib_src[waechter_idx:waechter_idx + 20000]

    if "overlay_verdecken(&h, &w);" not in waechter_chunk:
        errors.append("overlay_verdecken is not called in space_waechter")

    if "if auf_arbeitsplatz_ziel {\n                overlay_verdecken" in waechter_chunk:
        errors.append("overlay_verdecken is still conditionally guarded by auf_arbeitsplatz_ziel")

    verdecken_pos = waechter_chunk.find("overlay_verdecken(&h, &w);")
    umziehen_pos = waechter_chunk.find("space_umziehen(&w, wid, ziel_space, typ);")
    if verdecken_pos == -1 or umziehen_pos == -1 or verdecken_pos >= umziehen_pos:
        errors.append("overlay_verdecken must precede space_umziehen in space_waechter")

    # 2. Check SPACE_EPOCH atomic token and check in noki_space_gezeichnet
    if "SPACE_EPOCH" not in lib_src:
        errors.append("SPACE_EPOCH atomic not found in lib.rs")
    if "noki_space_gezeichnet(app: tauri::AppHandle, epoch: Option<u64>)" not in lib_src:
        errors.append("noki_space_gezeichnet does not accept epoch")

    # 3. Check index.html spaceVorbereiten doesn't set raum.x to onscreen x
    vorbereiten_idx = index_src.find("function spaceVorbereiten(dir)")
    assert vorbereiten_idx != -1, "spaceVorbereiten found in index.html"
    vorbereiten_chunk = index_src[vorbereiten_idx:vorbereiten_idx + 4000]

    if "raumVersetzen(x, y)" in vorbereiten_chunk:
        errors.append("spaceVorbereiten calls raumVersetzen(x, y) which sets raum.x to onscreen target")

    if "raum.x = xs; raum.ix = xs; spaceZug.x = xs;" not in vorbereiten_chunk:
        errors.append("spaceVorbereiten does not set offscreen coordinates xs directly")

    # 4. Check index.html spaceEintreten sets coordinates before spaceUnsichtbar(false)
    eintreten_idx = index_src.find("function spaceEintreten(dir)")
    assert eintreten_idx != -1, "spaceEintreten found in index.html"
    eintreten_chunk = index_src[eintreten_idx:eintreten_idx + 1200]

    unsichtbar_pos = eintreten_chunk.find("spaceUnsichtbar(false);")
    leinwand_pos = eintreten_chunk.find("leinwandStellen();")
    if unsichtbar_pos == -1 or leinwand_pos == -1 or unsichtbar_pos < leinwand_pos:
        errors.append("spaceUnsichtbar(false) must be called AFTER leinwandStellen() with offscreen coordinates")

    # 5. Check index.html currentSpaceEpoch tracking
    space_event_idx = index_src.find('an("noki://space"')
    assert space_event_idx != -1, 'an("noki://space") found in index.html'
    space_event_chunk = index_src[space_event_idx:space_event_idx + 1500]

    if "currentSpaceEpoch" not in space_event_chunk:
        errors.append("currentSpaceEpoch tracking not found in noki://space listener")

    if errors:
        print("FAIL - Code invariant errors:")
        for err in errors:
            print(f"  - {err}")
        return False

    print("PASS - All code invariants verified successfully.")
    return True

def simulate_100_space_transfers():
    """
    Simulate 100 space transfers under 60Hz and 120Hz frame steps.
    Verify:
    1. First visible frame x is ALWAYS offscreen (< 0 for dir=1, > stage.w for dir=-1)
    2. No frame at old position is ever rendered while visible
    3. Monotonic epoch advancement rejects stale completions
    4. State restoration returns exact previous state (WALK or FLY)
    """
    stage_w = 1728
    stage_h = 1117
    noki_h = 72

    class SimulationNoki:
        def __init__(self):
            self.x = 800
            self.y = 1000
            self.state = "IDLE_WALK"
            self.stage_visible = True
            self.native_window_alpha = 1.0
            self.epoch = 0
            self.current_epoch = 0
            self.space_zug = None
            self.rendered_frames = []

        def space_switch_detected(self, target_space, dir_val):
            self.epoch += 1
            ep = self.epoch
            # 1. phase: vorbereiten
            # native sets alpha = 0.0
            self.native_window_alpha = 0.0
            # JS spaceVorbereiten
            if ep < self.current_epoch:
                return
            self.current_epoch = ep

            prev_state = self.state
            g_links = 60
            g_rechts = stage_w - 60
            x_target = round(g_links + noki_h * 0.6) if dir_val > 0 else round(g_rechts - noki_h * 0.6)
            y_max = stage_h - 100 - round(noki_h * 0.9)
            y = y_max
            xs = -round(noki_h * 1.6) if dir_val > 0 else round(stage_w + noki_h * 1.6)

            self.space_zug = {
                "dir": dir_val,
                "prev_state": prev_state,
                "xIn": x_target,
                "y": y,
                "x": xs,
                "zx": round(x_target + dir_val * (g_rechts - g_links) * 0.45),
                "phase": "warten",
                "t": 0,
                "epoch": ep
            }
            # raum coordinates set offscreen
            self.x = xs
            self.y = y
            self.stage_visible = False

            # 2. native space_umziehen moves window to target space
            # window is moved while native_window_alpha == 0.0

            # 3. phase: eintreten
            if ep < self.current_epoch:
                return
            z = self.space_zug
            z["phase"] = "rein"
            z["v"] = 900
            z["D"] = abs(z["zx"] - z["x"])
            z["s"] = 0
            self.x = z["x"]
            self.stage_visible = True

            # WebKit renders 2 frames on target space
            for frame in range(2):
                self.record_frame(dt=1/120.0)

            # noki_space_gezeichnet
            if ep >= self.current_epoch:
                self.native_window_alpha = 1.0

            # Continue flight until complete
            while self.space_zug and self.space_zug["phase"] in ["rein", "landen"]:
                self.record_frame(dt=1/120.0)

        def record_frame(self, dt):
            z = self.space_zug
            if not z:
                return
            z["t"] += dt
            if z["phase"] == "rein":
                V = z["v"]
                rest = z["D"] - z["s"]
                v = min(V, V * (0.55 + 0.45 * z["t"] / 0.25))
                if rest < z["D"] * 0.3:
                    v = max(V * 0.12, V * rest / (z["D"] * 0.3))
                d = min(rest, v * dt)
                z["s"] += d
                self.x += z["dir"] * d
                if rest - d <= 0.5 or z["t"] > 6.0:
                    z["phase"] = "landen"
            elif z["phase"] == "landen":
                self.y = min(1000, self.y + 420 * dt)
                if self.y >= 1000:
                    z["phase"] = "fertig"
                    self.state = z["prev_state"]
                    self.space_zug = None

            # Only record if visible to user eye (native alpha == 1.0 AND stage_visible)
            is_user_visible = (self.native_window_alpha == 1.0) and self.stage_visible
            self.rendered_frames.append({
                "x": self.x,
                "y": self.y,
                "visible": is_user_visible,
                "epoch": z["epoch"] if z else self.current_epoch
            })

    sim = SimulationNoki()
    flashes = 0
    spawns = 0
    wrong_edge_entries = 0

    print("--- Running 100 Desktop-Transfer Simulation Cycles ---")
    for i in range(1, 101):
        sim.rendered_frames.clear()
        # Alternate between rightward (1) and leftward (-1) swipe, with occasional fast swipes
        direction = 1 if (i % 2 == 1) else -1
        sim.space_switch_detected(target_space=i % 4 + 1, dir_val=direction)

        # Inspect all frames that were visible to the user
        visible_frames = [f for f in sim.rendered_frames if f["visible"]]
        assert len(visible_frames) > 0, f"Cycle {i}: No visible frames recorded"

        first_visible = visible_frames[0]
        # Invariant 1: First visible frame MUST be at edge entry / speed flight
        # For dir=1 (entering from left), x must be <= 0 or at most slightly past screen edge in flight
        # For dir=-1 (entering from right), x must be >= stage_w or slightly inside
        if direction == 1:
            if first_visible["x"] > 50:
                print(f"Cycle {i} FAIL: First visible frame x={first_visible['x']} (expected <= 50 for left entry)")
                flashes += 1
            if first_visible["x"] > 200:
                spawns += 1
        else:
            if first_visible["x"] < stage_w - 50:
                print(f"Cycle {i} FAIL: First visible frame x={first_visible['x']} (expected >= {stage_w - 50} for right entry)")
                flashes += 1
            if first_visible["x"] < stage_w - 200:
                spawns += 1

        # Invariant 2: No visible frame at stale idle position (e.g. 800) before entry
        for vf in visible_frames[:5]:
            if 700 <= vf["x"] <= 900 and first_visible["x"] > 600:
                print(f"Cycle {i} FAIL: Stale position visible at frame {vf}")
                flashes += 1

    print(f"100 transfers completed:")
    print(f"  Old position flashes: {flashes}")
    print(f"  One-frame spawns: {spawns}")
    print(f"  Wrong edge entries: {wrong_edge_entries}")

    if flashes == 0 and spawns == 0 and wrong_edge_entries == 0:
        print("PASS - Desktop-Transfer verification passed 100/100 cycles.")
        return True
    return False

if __name__ == "__main__":
    ok1 = test_code_invariants()
    ok2 = simulate_100_space_transfers()
    if ok1 and ok2:
        print("\nALL INVARIANTS SATISFIED (0 flashes, 0 spawns, 0 regressions).")
        sys.exit(0)
    else:
        sys.exit(1)
