#!/usr/bin/env python3
"""Five difficult Chat and Code cases, same qwen3.5:9b with thinking off/on."""
import json, time
from pathlib import Path
from ollama_chat_ab import CASES, api, loaded, unload, prompt, generate, score, rss_mb, swap_mb
from ollama_code_ab import TASKS
from ollama_code_agent_ab import task_run

MODEL='qwen3.5:9b'
CHAT_IDS=(7,8,11,18,24)
CODE_IDS=(2,3,9,17,19)
OUT=Path('.local/qa/qwen35-thinking-subset.json')

def main():
    result={'model':MODEL,'chat_ids':[i+1 for i in CHAT_IDS],'code_ids':[i+1 for i in CODE_IDS],'modes':[]}
    for thinking in (False,True):
        if loaded():
            for m in loaded():
                if m!=MODEL:raise RuntimeError('Other loaded model; will not touch: '+m)
                unload(m)
        swap0=swap_mb();api('/api/generate',{'model':MODEL,'prompt':'','stream':False,'keep_alive':'10m'});rows=[];peak=rss_mb()
        for i in CHAT_IDS:
            c=CASES[i];v,sec,mem=generate(MODEL,prompt(c),max_tokens=600,thinking=thinking);answer=v.get('response','');peak=max(peak,mem)
            rows.append({'id':i+1,'score':score(c,answer),'answer':answer,'latency_ms':round(sec*1000),'tokens_per_second':round(v.get('eval_count',0)/(v.get('eval_duration',1)/1e9),2)})
        code=[]
        for i in CODE_IDS:
            code.append(task_run(MODEL,i,TASKS[i],thinking));peak=max(peak,rss_mb())
        result['modes'].append({'thinking':thinking,'chat':rows,'code':code,'peak_ram_mb':round(peak),'swap_delta_mb':round(swap_mb()-swap0,1)})
        unload(MODEL);OUT.parent.mkdir(parents=True,exist_ok=True);OUT.write_text(json.dumps(result,ensure_ascii=False,indent=2));print('thinking=',thinking,'chat=',len(rows),'code=',len(code),flush=True)
if __name__=='__main__':main()
