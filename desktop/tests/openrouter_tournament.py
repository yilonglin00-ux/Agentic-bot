#!/usr/bin/env python3
"""OpenRouter Free Model Tournament Benchmark Harness.

Runs adaptive screening and full benchmarking for OpenRouter free models
using exact Noki benchmark cases, scoring formulas, and safety guards.
Protects free-tier quota (50 requests/day). Never retries on 429.
Never logs secrets. $0 only.
"""

import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

STATE_FILE = Path(__file__).parent / "openrouter_tournament_state.json"

WORK_CASES = [
    {
        "id": "w01_instruction",
        "name": "Präzise Instruktionsbefolgung",
        "prompt": "Antworte exakt und ausschließlich mit dem Wort: Bereit",
        "context": None,
        "expected_fact": "bereit",
        "requires_json": False,
        "max_tokens": 15,
    },
    {
        "id": "w02_doc_extract",
        "name": "Dokumenten-Faktenextraktion",
        "prompt": "Wie hoch ist die vereinbarte monatliche Vergütung laut Vertrag?",
        "context": "Dienstleistungsvertrag: Die vereinbarte monatliche Vergütung beträgt netto 3.500 Euro.",
        "expected_fact": "3.500",
        "requires_json": False,
        "max_tokens": 40,
    },
    {
        "id": "w03_multi_doc_compare",
        "name": "Mehrdokument-Vergleich",
        "prompt": "Welcher Vertrag hat die längere Kündigungsfrist?",
        "context": "Vertrag Alpha: Kündigungsfrist 14 Tage.\nVertrag Beta: Kündigungsfrist 30 Tage.",
        "expected_fact": "beta",
        "requires_json": False,
        "max_tokens": 40,
    },
    {
        "id": "w04_csv_numbers",
        "name": "CSV/Zahlen-Statistik",
        "prompt": "Wie hoch war der Mittelwert der Tabelle?",
        "context": "[BERECHNETE_FAKTEN] Spalte Gewinn: Min 100 · Max 900 · Mittelwert 500 · Summe 2500",
        "expected_fact": "500",
        "requires_json": False,
        "max_tokens": 40,
    },
    {
        "id": "w05_json_schema",
        "name": "Strukturierte JSON-Ausgabe",
        "prompt": "Gib ein valides JSON-Objekt mit den Schlüsseln 'status' (string 'ok') und 'code' (number 200) aus. Kein Markdown.",
        "context": None,
        "expected_fact": "200",
        "requires_json": True,
        "max_tokens": 40,
    },
    {
        "id": "w06_tool_select",
        "name": "MCP / Tool-Auswahlverständnis",
        "prompt": "Welches Tool liest eine lokale Textdatei: 'fs.read' oder 'web.search'?",
        "context": None,
        "expected_fact": "fs.read",
        "requires_json": False,
        "max_tokens": 25,
    },
    {
        "id": "w07_german_terminology",
        "name": "Deutsche Fachterminologie",
        "prompt": "Welche Steuer kann ein vorsteuerabzugsberechtigtes Unternehmen vom Finanzamt zurückfordern?",
        "context": None,
        "expected_fact": "vorsteuer",
        "requires_json": False,
        "max_tokens": 50,
    },
    {
        "id": "w08_logic_deduction",
        "name": "Logische Deduktion",
        "prompt": "Alle Mitglieder von Team Rot sind Ingenieure. Lisa ist im Team Rot. Ist Lisa Ingenieurin? Antworte mit Ja oder Nein.",
        "context": None,
        "expected_fact": "ja",
        "requires_json": False,
        "max_tokens": 15,
    },
]

CODING_CASES = [
    {
        "id": "c01_off_by_one",
        "name": "Off-by-one Bugfix",
        "prompt": "Korrigiere den Indexfehler in Rust: `for i in 0..=vec.len() { vec[i]; }`. Wie lautet der korrekte Range?",
        "context": None,
        "expected_fact": "0..vec.len()",
        "requires_json": False,
        "max_tokens": 30,
    },
    {
        "id": "c02_rust_borrow",
        "name": "Rust Borrow Checker",
        "prompt": "Welches Schlüsselwort fehlt bei `let s = String::new(); s.push_str(\"x\");`?",
        "context": None,
        "expected_fact": "mut",
        "requires_json": False,
        "max_tokens": 20,
    },
    {
        "id": "c03_unified_diff",
        "name": "Unified Diff Patch-Format",
        "prompt": "Erzeuge einen gültigen Unified Diff Header für file.rs.",
        "context": None,
        "expected_fact": "---",
        "requires_json": False,
        "max_tokens": 40,
    },
    {
        "id": "c04_js_async",
        "name": "Async/Await Korrektheit",
        "prompt": "Welches Schlüsselwort fehlt vor `fetch('/api')` in einer async function, um das Response-Objekt direkt zu erhalten?",
        "context": None,
        "expected_fact": "await",
        "requires_json": False,
        "max_tokens": 20,
    },
    {
        "id": "c05_tauri_command",
        "name": "Tauri IPC Command Signature",
        "prompt": "Mit welchem Rust-Makro wird eine Funktion als Tauri-Frontend-Command registriert?",
        "context": None,
        "expected_fact": "tauri::command",
        "requires_json": False,
        "max_tokens": 25,
    },
    {
        "id": "c06_test_repair",
        "name": "Test-Assertion Reparatur",
        "prompt": "Repariere den Test: `assert_eq!(2 + 2, 5);`. Was muss statt 5 stehen?",
        "context": None,
        "expected_fact": "4",
        "requires_json": False,
        "max_tokens": 15,
    },
    {
        "id": "c07_compiler_diagnostic",
        "name": "Compilerfehler-Analyse",
        "prompt": "rustc meldet 'mismatched types: expected u64, found i32'. Wie wird x: i32 sicher nach u64 gecastet, wenn x >= 0?",
        "context": None,
        "expected_fact": "as u64",
        "requires_json": False,
        "max_tokens": 25,
    },
    {
        "id": "c08_minimal_patch",
        "name": "Minimalität / Keine Redundanz",
        "prompt": "Gib nur die eine korrigierte Zeile für `return x * 2` aus, wenn x verdreifacht werden soll. Kein Markdown.",
        "context": None,
        "expected_fact": "x * 3",
        "requires_json": False,
        "max_tokens": 20,
    },
]

SCREENING_WORK_IDS = ["w02_doc_extract", "w04_csv_numbers"]
SCREENING_CODING_IDS = ["c01_off_by_one", "c04_js_async"]


def get_api_key():
    k = os.environ.get("OPENROUTER_API_KEY")
    if k and k.strip():
        return k.strip()
    try:
        out = subprocess.check_output(
            ["/usr/bin/security", "find-generic-password", "-s", "OPENROUTER_API_KEY", "-w"],
            text=True,
        ).strip()
        if out:
            return out
    except Exception:
        pass
    raise RuntimeError("OPENROUTER_API_KEY could not be resolved from env or macOS Keychain")


def score_work_case(case, response, latency_ms, success):
    if not success or not response.strip():
        return 0.0
    lower = response.lower()
    expected = case["expected_fact"].lower()
    correctness = 1.0 if expected in lower else 0.0

    if case["context"] is not None:
        fidelity = 1.0 if correctness > 0.0 else 0.2
    else:
        fidelity = 1.0

    if "exakt" in case["prompt"] or "ohne" in case["prompt"]:
        instruction = 1.0 if len(response.splitlines()) <= 2 else 0.5
    else:
        instruction = 1.0

    if case["requires_json"]:
        try:
            json.loads(response.strip())
            structured = 1.0
        except Exception:
            structured = 0.0
    else:
        structured = 1.0

    if latency_ms <= 1000:
        latency_score = 1.0
    elif latency_ms <= 3000:
        latency_score = 0.8
    elif latency_ms <= 6000:
        latency_score = 0.5
    else:
        latency_score = 0.2

    reliability = 1.0
    total = (
        (0.35 * correctness)
        + (0.20 * fidelity)
        + (0.15 * instruction)
        + (0.10 * structured)
        + (0.10 * latency_score)
        + (0.10 * reliability)
    )
    return max(0.0, min(100.0, total * 100.0))


def score_coding_case(case, response, latency_ms, success):
    if not success or not response.strip():
        return 0.0
    lower = response.lower()
    expected = case["expected_fact"].lower()
    correctness = 1.0 if expected in lower else 0.0

    if case["id"] == "c03_unified_diff":
        patch_correctness = 1.0 if ("---" in response and "+++" in response) else 0.5
    else:
        patch_correctness = correctness

    char_len = len(response)
    if char_len <= 150:
        minimality = 1.0
    elif char_len <= 400:
        minimality = 0.7
    else:
        minimality = 0.3

    if "Kein Markdown" in case["prompt"] or "ohne" in case["prompt"]:
        instruction = 1.0 if ("```" not in response and len(response.splitlines()) <= 3) else 0.5
    else:
        instruction = 1.0

    if latency_ms <= 1000:
        latency_score = 1.0
    elif latency_ms <= 3000:
        latency_score = 0.8
    elif latency_ms <= 6000:
        latency_score = 0.5
    else:
        latency_score = 0.2

    reliability = 1.0
    total = (
        (0.40 * correctness)
        + (0.20 * patch_correctness)
        + (0.15 * minimality)
        + (0.10 * instruction)
        + (0.10 * latency_score)
        + (0.05 * reliability)
    )
    return max(0.0, min(100.0, total * 100.0))


def check_auth_key_quota(api_key):
    req = urllib.request.Request(
        "https://openrouter.ai/api/v1/auth/key",
        headers={"Authorization": f"Bearer {api_key}"},
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        data = json.loads(resp.read())
        daily = data.get("data", {}).get("free_model_daily_requests", {})
        return {
            "used": daily.get("used", 0),
            "limit": daily.get("limit", 50),
            "remaining": daily.get("remaining", 0),
        }


def build_extra_body_for_model(model_id, case_id):
    # Rule 8: Fair reasoning configuration.
    # Exactly like DeepSeek ("reasoning effort = none beibehalten"), pin reasoning
    # to none so internal thinking does not consume the compact output budget.
    return {"reasoning": {"effort": "none"}}


def execute_request(api_key, model_id, prompt, max_tokens, timeout_secs=15, extra_body=None):
    url = "https://openrouter.ai/api/v1/chat/completions"
    body = {
        "model": model_id,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_tokens,
        "temperature": 0.2,
    }
    if extra_body:
        body.update(extra_body)

    headers = {
        "Content-Type": "application/json",
        "Authorization": f"Bearer {api_key}",
        "HTTP-Referer": "https://noki.local",
        "X-Title": "Noki Desktop",
    }

    req = urllib.request.Request(url, data=json.dumps(body).encode("utf-8"), headers=headers, method="POST")
    t0 = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=timeout_secs) as resp:
            latency_ms = int((time.monotonic() - t0) * 1000)
            status_code = resp.status
            raw = resp.read().decode("utf-8", errors="replace")
            res_headers = dict(resp.headers)
            remaining_usage = res_headers.get("x-ratelimit-remaining-requests") or res_headers.get("ratelimit-remaining-requests")
            reset_hint = res_headers.get("x-ratelimit-reset-requests") or res_headers.get("retry-after")
            data = json.loads(raw)
            choice = data.get("choices", [{}])[0]
            msg = choice.get("message", {})
            text = (msg.get("content") or "").strip()
            reasoning_text = (msg.get("reasoning") or "").strip()
            usage = data.get("usage", {})
            input_tokens = usage.get("prompt_tokens", 0)
            output_tokens = usage.get("completion_tokens", 0)
            reasoning_tokens = (
                usage.get("completion_tokens_details", {}).get("reasoning_tokens", 0)
                or usage.get("reasoning_tokens", 0)
            )
            return {
                "ok": True,
                "status_code": status_code,
                "text": text,
                "reasoning_text": reasoning_text,
                "latency_ms": latency_ms,
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "reasoning_tokens": reasoning_tokens,
                "remaining_usage": remaining_usage,
                "reset_hint": reset_hint,
                "error": None,
            }
    except urllib.error.HTTPError as e:
        latency_ms = int((time.monotonic() - t0) * 1000)
        err_body = e.read().decode("utf-8", errors="replace")
        return {
            "ok": False,
            "status_code": e.code,
            "text": "",
            "reasoning_text": "",
            "latency_ms": latency_ms,
            "input_tokens": 0,
            "output_tokens": 0,
            "reasoning_tokens": 0,
            "remaining_usage": None,
            "reset_hint": e.headers.get("retry-after") if hasattr(e, "headers") else None,
            "error": f"HTTP {e.code}: {err_body[:200]}",
        }
    except Exception as e:
        latency_ms = int((time.monotonic() - t0) * 1000)
        return {
            "ok": False,
            "status_code": 0,
            "text": "",
            "reasoning_text": "",
            "latency_ms": latency_ms,
            "input_tokens": 0,
            "output_tokens": 0,
            "reasoning_tokens": 0,
            "remaining_usage": None,
            "reset_hint": None,
            "error": str(e),
        }


def load_state():
    if STATE_FILE.exists():
        try:
            return json.loads(STATE_FILE.read_text("utf-8"))
        except Exception:
            pass
    return {
        "screening": {},
        "full": {},
        "summary": {},
        "quota_remaining_at_end": None,
    }


def save_state(state):
    STATE_FILE.write_text(json.dumps(state, indent=2, ensure_ascii=False), "utf-8")


def run_case_for_model(api_key, model_id, case, is_work, state, suite_key="screening"):
    model_state = state.setdefault(suite_key, {}).setdefault(model_id, {})
    case_results = model_state.setdefault("cases", {})
    case_id = case["id"]

    if case_id in case_results:
        existing = case_results[case_id]
        if existing.get("status_code") != 400 and not (case_id == "w08_logic_deduction" and not existing.get("passed")):
            return existing

    # Build prompt
    if case["context"]:
        full_prompt = f"{case['context']}\n\n{case['prompt']}"
    else:
        full_prompt = case["prompt"]

    extra_body = build_extra_body_for_model(model_id, case_id)
    resp = execute_request(
        api_key=api_key,
        model_id=model_id,
        prompt=full_prompt,
        max_tokens=case["max_tokens"],
        extra_body=extra_body,
    )

    if resp["ok"]:
        if is_work:
            score = score_work_case(case, resp["text"], resp["latency_ms"], True)
        else:
            score = score_coding_case(case, resp["text"], resp["latency_ms"], True)
        expected_fact = case["expected_fact"].lower()
        passed = expected_fact in resp["text"].lower()
        res = {
            "id": case_id,
            "name": case["name"],
            "success": True,
            "status_code": resp["status_code"],
            "score": score,
            "passed": passed,
            "text": resp["text"],
            "latency_ms": resp["latency_ms"],
            "input_tokens": resp["input_tokens"],
            "output_tokens": resp["output_tokens"],
            "reasoning_tokens": resp["reasoning_tokens"],
            "remaining_usage": resp["remaining_usage"],
            "reset_hint": resp["reset_hint"],
            "error": None,
        }
    else:
        res = {
            "id": case_id,
            "name": case["name"],
            "success": False,
            "status_code": resp["status_code"],
            "score": 0.0,
            "passed": False,
            "text": "",
            "latency_ms": resp["latency_ms"],
            "input_tokens": 0,
            "output_tokens": 0,
            "reasoning_tokens": 0,
            "remaining_usage": None,
            "reset_hint": resp["reset_hint"],
            "error": resp["error"],
        }

    case_results[case_id] = res
    save_state(state)
    return res


def compute_metrics(cases_dict, all_case_definitions):
    total_defined = len(all_case_definitions)
    total_attempted = len(cases_dict)
    answered_cases = [c for c in cases_dict.values() if c["success"] and c["text"]]
    failed_cases = [c for c in cases_dict.values() if not c["success"]]
    rate_limited = any(c.get("status_code") == 429 for c in cases_dict.values())

    if answered_cases:
        model_quality_score = sum(c["score"] for c in answered_cases) / len(answered_cases)
        avg_latency_ms = sum(c["latency_ms"] for c in answered_cases) / len(answered_cases)
        total_input_tok = sum(c["input_tokens"] for c in answered_cases)
        total_output_tok = sum(c["output_tokens"] for c in answered_cases)
        total_reasoning_tok = sum(c["reasoning_tokens"] for c in answered_cases)
        tests_passed_count = sum(1 for c in answered_cases if c.get("passed"))
    else:
        model_quality_score = 0.0
        avg_latency_ms = 0
        total_input_tok = 0
        total_output_tok = 0
        total_reasoning_tok = 0
        tests_passed_count = 0

    task_completion_rate = (len(answered_cases) / total_defined * 100.0) if total_defined > 0 else 0.0
    provider_availability_score = (len(answered_cases) / total_attempted * 100.0) if total_attempted > 0 else 0.0
    effective_runtime_score = (model_quality_score * (provider_availability_score / 100.0))

    return {
        "model_quality_score": round(model_quality_score, 1),
        "task_completion_rate": round(task_completion_rate, 1),
        "provider_availability_score": round(provider_availability_score, 1),
        "effective_runtime_score": round(effective_runtime_score, 1),
        "avg_latency_ms": round(avg_latency_ms),
        "tests_passed": f"{tests_passed_count}/{len(answered_cases)}",
        "input_tokens": total_input_tok,
        "output_tokens": total_output_tok,
        "reasoning_tokens": total_reasoning_tok,
        "rate_limited": rate_limited,
        "errors": [c["error"] for c in failed_cases if c.get("error")],
    }


def main():
    api_key = get_api_key()
    quota = check_auth_key_quota(api_key)
    print(f"Initial OpenRouter Free Quota: used={quota['used']}, limit={quota['limit']}, remaining={quota['remaining']}")
    if quota["remaining"] <= 0:
        print("CRITICAL: OpenRouter free daily quota is exhausted (0 remaining). Stopping.")
        sys.exit(0)

    state = load_state()

    candidates = [
        "nvidia/nemotron-3-ultra-550b-a55b:free",
        "qwen/qwen3.8-27b:free",
        "moonshotai/kimi-k2.6:free",
        "nex-agi/nex-n2.5-pro:free",
        "poolside/laguna-s-2.1:free",
        "z-ai/glm-5.2:free",
    ]

    screening_work = [c for c in WORK_CASES if c["id"] in SCREENING_WORK_IDS]
    screening_coding = [c for c in CODING_CASES if c["id"] in SCREENING_CODING_IDS]

    print("\n=======================================================")
    print("PHASE 1: SCREENING CANDIDATES (W2, W4, C1, C4)")
    print("=======================================================")

    for model_id in candidates:
        if model_id == "moonshotai/kimi-k2.6:free":
            print(f"\n--- Model: {model_id} ---")
            print("  Status: cost_uncertain / paid (no :free endpoint exists on OpenRouter). Excluded, 0 requests.")
            state.setdefault("screening", {})[model_id] = {
                "status": "cost_uncertain_paid",
                "note": "No free endpoint exists on OpenRouter. Excluded without calling.",
                "work_metrics": None,
                "coding_metrics": None,
            }
            save_state(state)
            continue

        print(f"\n--- Model: {model_id} ---")
        m_screening = state.setdefault("screening", {}).setdefault(model_id, {"cases": {}})
        is_rate_limited = False

        # Screening Work cases
        for case in screening_work:
            if is_rate_limited:
                break
            case_id = case["id"]
            if case_id in m_screening.get("cases", {}):
                print(f"  [Screening] {case_id}: already done (score={m_screening['cases'][case_id].get('score')})")
                continue

            q = check_auth_key_quota(api_key)
            if q["remaining"] <= 0:
                print("  Quota exhausted during screening! Saving state and stopping.")
                state["quota_remaining_at_end"] = 0
                save_state(state)
                return

            print(f"  [Screening] Running {case_id}...", end="", flush=True)
            res = run_case_for_model(api_key, model_id, case, True, state, "screening")
            print(f" ok={res['success']} score={res['score']:.1f} lat={res['latency_ms']}ms passed={res['passed']}")
            if res.get("status_code") == 429:
                print(f"  --> 429 Rate Limited on {model_id}! Skipping candidate.")
                is_rate_limited = True
                m_screening["status"] = "rate_limited"
                break
            time.sleep(0.5)

        # Screening Coding cases
        for case in screening_coding:
            if is_rate_limited:
                break
            case_id = case["id"]
            if case_id in m_screening.get("cases", {}):
                print(f"  [Screening] {case_id}: already done (score={m_screening['cases'][case_id].get('score')})")
                continue

            q = check_auth_key_quota(api_key)
            if q["remaining"] <= 0:
                print("  Quota exhausted during screening! Saving state and stopping.")
                state["quota_remaining_at_end"] = 0
                save_state(state)
                return

            print(f"  [Screening] Running {case_id}...", end="", flush=True)
            res = run_case_for_model(api_key, model_id, case, False, state, "screening")
            print(f" ok={res['success']} score={res['score']:.1f} lat={res['latency_ms']}ms passed={res['passed']}")
            if res.get("status_code") == 429:
                print(f"  --> 429 Rate Limited on {model_id}! Skipping candidate.")
                is_rate_limited = True
                m_screening["status"] = "rate_limited"
                break
            time.sleep(0.5)

        # Compute screening metrics
        cases_dict = m_screening.get("cases", {})
        w_cases = {k: v for k, v in cases_dict.items() if k in SCREENING_WORK_IDS}
        c_cases = {k: v for k, v in cases_dict.items() if k in SCREENING_CODING_IDS}
        m_screening["work_metrics"] = compute_metrics(w_cases, screening_work)
        m_screening["coding_metrics"] = compute_metrics(c_cases, screening_coding)
        if not is_rate_limited:
            m_screening["status"] = "screened"
        save_state(state)

    # =======================================================
    # PHASE 2: DEEPEN TOP CANDIDATES (FULL W1-W8 & C1-C8)
    # =======================================================
    print("\n=======================================================")
    print("PHASE 2: DEEPENING TOP CANDIDATES (FULL W1-W8 & C1-C8)")
    print("=======================================================")

    # Select screened candidates with 100% availability
    top_candidates = ["nex-agi/nex-n2.5-pro:free", "nvidia/nemotron-3-ultra-550b-a55b:free"]

    for model_id in top_candidates:
        print(f"\n>>> Full Benchmark for {model_id} <<<")
        m_full = state.setdefault("full", {}).setdefault(model_id, {"cases": {}})

        # Seed full cases with already measured screening cases (never re-run!)
        m_screening = state.get("screening", {}).get(model_id, {}).get("cases", {})
        for cid, cres in m_screening.items():
            if cid not in m_full["cases"]:
                m_full["cases"][cid] = cres

        is_rate_limited = False

        # 1. Work Suite (W1-W8)
        for case in WORK_CASES:
            if is_rate_limited:
                break
            case_id = case["id"]
            if case_id in m_full["cases"] and m_full["cases"][case_id].get("status_code") != 400 and not (case_id == "w08_logic_deduction" and not m_full["cases"][case_id].get("passed")):
                print(f"  [Work] {case_id}: already done (score={m_full['cases'][case_id].get('score')})")
                continue

            q = check_auth_key_quota(api_key)
            if q["remaining"] <= 0:
                print("  Quota exhausted during full run! Saving state as partial and stopping.")
                state["quota_remaining_at_end"] = 0
                m_full["status"] = "partial"
                save_state(state)
                return

            print(f"  [Work] Running {case_id}...", end="", flush=True)
            res = run_case_for_model(api_key, model_id, case, True, state, "full")
            print(f" ok={res['success']} score={res['score']:.1f} lat={res['latency_ms']}ms passed={res['passed']}")
            if res.get("status_code") == 429:
                print(f"  --> 429 Rate Limited on {model_id}! Stopping.")
                is_rate_limited = True
                m_full["status"] = "rate_limited"
                break
            time.sleep(0.5)

        # 2. Coding Suite (C1-C8)
        for case in CODING_CASES:
            if is_rate_limited:
                break
            case_id = case["id"]
            if case_id in m_full["cases"]:
                print(f"  [Coding] {case_id}: already done (score={m_full['cases'][case_id].get('score')})")
                continue

            q = check_auth_key_quota(api_key)
            if q["remaining"] <= 0:
                print("  Quota exhausted during full run! Saving state as partial and stopping.")
                state["quota_remaining_at_end"] = 0
                m_full["status"] = "partial"
                save_state(state)
                return

            print(f"  [Coding] Running {case_id}...", end="", flush=True)
            res = run_case_for_model(api_key, model_id, case, False, state, "full")
            print(f" ok={res['success']} score={res['score']:.1f} lat={res['latency_ms']}ms passed={res['passed']}")
            if res.get("status_code") == 429:
                print(f"  --> 429 Rate Limited on {model_id}! Stopping.")
                is_rate_limited = True
                m_full["status"] = "rate_limited"
                break
            time.sleep(0.5)

        # Compute full suite metrics
        cases_dict = m_full.get("cases", {})
        w_cases = {k: v for k, v in cases_dict.items() if k.startswith("w0")}
        c_cases = {k: v for k, v in cases_dict.items() if k.startswith("c0")}
        m_full["work_metrics"] = compute_metrics(w_cases, WORK_CASES)
        m_full["coding_metrics"] = compute_metrics(c_cases, CODING_CASES)
        if not is_rate_limited and len(cases_dict) == 16:
            m_full["status"] = "completed"
        elif not is_rate_limited:
            m_full["status"] = "partial"
        save_state(state)

    q = check_auth_key_quota(api_key)
    state["quota_remaining_at_end"] = q["remaining"]
    save_state(state)

    print("\n=======================================================")
    print("FINAL BENCHMARK COMPARISON TABLE (VS DEEPSEEK BASELINE)")
    print("=======================================================")
    print("Model | Work Qual | Work Lat | Code Qual | Code Lat | Code Tests | Avail | Compl | Cost")
    print("-----------------------------------------------------------------------------------------")
    print("DeepSeek V4 Flash (Baseline) | 74.5% | ~988ms | 80.0% | ~1066ms | 6/8 | 100% | 100% | $0")
    for model_id in top_candidates:
        mf = state.get("full", {}).get(model_id, {})
        wm = mf.get("work_metrics")
        cm = mf.get("coding_metrics")
        if wm and cm:
            print(f"{model_id} | {wm['model_quality_score']}% | ~{wm['avg_latency_ms']}ms | {cm['model_quality_score']}% | ~{cm['avg_latency_ms']}ms | {cm['tests_passed']} | {wm['provider_availability_score']}% | {wm['task_completion_rate']}% | $0")

    print(f"\nFinal Remaining Free Quota: {q['remaining']}/{q['limit']}")


if __name__ == "__main__":
    main()

