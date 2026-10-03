#!/usr/bin/env python3
"""Three identical longform Noki prompts, scored conservatively for length and grounding."""
import argparse, json, re
from pathlib import Path
from ollama_chat_ab import api, loaded, unload, rss_mb, swap_mb, pressure_free_pct, prompt, generate

CASES = [
    ("longform", "Schreibe etwa 1000 Wörter auf Deutsch für eine 12-Jährige: Was sind CPU, RAM und SSD, wie arbeiten sie zusammen, und wann bringt mehr RAM etwas? Nutze klare Zwischenüberschriften, zwei konkrete Alltagsszenen und eine kurze Zusammenfassung. Erfinde keine aktuellen Preise oder Benchmarks.", [], [], 1000),
    ("longform", "Schreibe etwa 1000 Wörter auf Deutsch als praktische Anleitung: Ein MacBook mit 16 GB RAM wird bei vielen Browser-Tabs langsam. Erkläre die möglichen Ursachen, eine sichere Diagnose in sechs Schritten und die Grenzen der Diagnose. Keine erfundenen Messwerte, keine pauschale Behauptung, dass RAM immer schuld ist.", [], [], 1000),
    ("longform", "EVIDENZ: Quelle A meldet 80 mg Koffein je Dose. Quelle B nennt 400 mg pro Tag als Richtwert für gesunde Erwachsene. Weitere Daten zu Produkt, Person und Gesundheit liegen nicht vor. Schreibe etwa 1000 Wörter auf Deutsch: ordne die Rechnung ein, erkläre Unsicherheit und nenne, welche Angaben für eine persönliche Einschätzung fehlen. Nutze nur diese Evidenz als Tatsachengrundlage; gib keine persönliche medizinische Empfehlung.", [], [], 1000),
]

def run(model):
    managed={"qwen2.5:3b-instruct-q5_K_M","qwen3.5:4b","qwen3.5:9b"}
    for m in loaded():
        if m in managed: unload(m)
    if loaded(): raise RuntimeError("Foreign model loaded: "+str(loaded()))
    before=swap_mb(); pressure=pressure_free_pct(); rows=[]; peak=0
    for i,c in enumerate(CASES,1):
        v,sec,ram=generate(model,prompt(c),max_tokens=2700); answer=v.get("response",""); words=len(re.findall(r"\b[\wÄÖÜäöüß-]+\b",answer)); peak=max(peak,ram)
        forbidden=bool(re.search(r"\b(?:quelle c|studie (?:beweist|zeigt)|\d+[,.]\d+\s*€)\b",answer,re.I))
        rows.append({"id":i,"words":words,"length_ok":800<=words<=1200,"unsupported_marker":forbidden,"latency_ms":round(sec*1000),"tokens_per_second":round(v.get("eval_count",0)/(v.get("eval_duration",1)/1e9),2),"answer":answer})
    unload(model)
    return {"model":model,"cases":3,"rows":rows,"peak_ram_mb":round(peak),"swap_delta_mb":round(swap_mb()-before,1),"memory_free_pct_before":pressure,"memory_free_pct_after":pressure_free_pct()}

def main():
    p=argparse.ArgumentParser();p.add_argument("--models",nargs="+",required=True);p.add_argument("--out",required=True);a=p.parse_args()
    dest=Path(a.out);dest.parent.mkdir(parents=True,exist_ok=True);result={"models":[]}
    for model in a.models:
        result["models"].append(run(model));dest.write_text(json.dumps(result,ensure_ascii=False,indent=2));print(model,[(r["words"],r["length_ok"]) for r in result["models"][-1]["rows"]],flush=True)
if __name__=="__main__":main()
