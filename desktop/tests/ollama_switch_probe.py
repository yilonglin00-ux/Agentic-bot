#!/usr/bin/env python3
"""Verifies unload -> absent -> load and records RAM/pressure for each mode switch."""
import argparse,json,re,subprocess,time,urllib.request
from pathlib import Path
def api(path,p=None):
 d=None if p is None else json.dumps(p).encode();q=urllib.request.Request('http://127.0.0.1:11434'+path,data=d,headers={'Content-Type':'application/json'},method='GET' if d is None else 'POST');return json.load(urllib.request.urlopen(q,timeout=900))
def loaded():return [m.get('name',m.get('model','')) for m in api('/api/ps').get('models',[])]
def rss():
 out=subprocess.check_output(['ps','-axo','rss=,command='],text=True);return round(sum(int(x.split(None,1)[0]) for x in out.splitlines() if 'ollama runner' in x or '/Applications/Ollama.app/' in x)/1024)
def swap():
 s=subprocess.check_output(['/usr/sbin/sysctl','-n','vm.swapusage'],text=True);m=re.search(r'used = ([\d.]+)([MG])',s);return round(float(m.group(1))*(1024 if m and m.group(2)=='G' else 1),1) if m else 0
def pressure():
 s=subprocess.check_output(['/usr/bin/memory_pressure'],text=True);m=re.search(r'System-wide memory free percentage:\s*(\d+)%',s);return int(m.group(1)) if m else None
def unload_all():
 managed={'qwen2.5:3b-instruct-q5_K_M','qwen3.5:4b','qwen3.5:9b','qwen2.5-coder:7b','qwen2.5-coder:14b','qwen2.5:1.5b','qwen3:4b'}
 foreign=[m for m in loaded() if m not in managed]
 if foreign:raise RuntimeError('Foreign loaded model; switch probe will not touch it: '+str(foreign))
 for m in loaded():api('/api/generate',{'model':m,'prompt':'','stream':False,'keep_alive':0})
 for _ in range(50):
  if not loaded():return
  time.sleep(.1)
 raise RuntimeError('models still loaded: '+str(loaded()))
def main():
 p=argparse.ArgumentParser();p.add_argument('--models',nargs='+',required=True);p.add_argument('--out',default='.local/qa/model-switch.json');a=p.parse_args();unload_all();rows=[];mx=0
 for model in a.models:
  before={'loaded':loaded(),'rss_mb':rss(),'swap_mb':swap(),'memory_free_pct':pressure()};t=time.monotonic();v=api('/api/generate',{'model':model,'prompt':'Antworte nur OK.','stream':False,'keep_alive':'10m','options':{'num_predict':8,'temperature':0}});during=loaded();mx=max(mx,len(during));loaded_metrics={'loaded':during,'rss_mb':rss(),'swap_mb':swap(),'memory_free_pct':pressure(),'load_and_probe_ms':round((time.monotonic()-t)*1000),'tokens_per_second':round(v.get('eval_count',0)/(v.get('eval_duration',1)/1e9),2)};unload_all();time.sleep(1);after={'loaded':loaded(),'rss_mb':rss(),'swap_mb':swap(),'memory_free_pct':pressure()};rows.append({'model':model,'before':before,'active':loaded_metrics,'after_unload':after,'inactive_model_loaded':bool(after['loaded'])})
 out={'switches':rows,'loaded_models_max':mx,'pass':mx<=1 and all(not r['inactive_model_loaded'] for r in rows)};Path(a.out).parent.mkdir(parents=True,exist_ok=True);Path(a.out).write_text(json.dumps(out,indent=2));print(json.dumps(out,indent=2))
if __name__=='__main__':main()
