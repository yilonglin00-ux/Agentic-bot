#!/usr/bin/env python3
"""20-task local coding-model A/B with executable Rust/JS fixtures."""
import argparse,json,os,re,subprocess,tempfile,threading,time,urllib.request
from pathlib import Path

TASKS=[
 ("repo_understanding","src/lib.rs","Rust repo: src/lib.rs owns commands; web/app.js owns UI. Which file should register a new Tauri command? Return that file and a minimal compilable Rust function main printing 'src/lib.rs'.","src/lib.rs","rust"),
 ("find_file","web/state.js","Files: web/view.css styles UI; web/state.js owns state; src/lib.rs backend. Fix JS state update. Return target file with complete JS: let state={n:0}; function inc(){state.n+=1}; inc(); if(state.n!==1)throw Error();","state.n","js"),
 ("bug_localize","src/lib.rs","Fix off-by-one. Return complete Rust file with fn last(a:&[i32])->Option<i32> returning final element and a passing test.",r"\.last\(","rust"),
 ("rust_compile","src/main.rs","Fix compile error: fn main(){ let x: i32 = \"3\"; println!(\"{}\",x); }. Return complete compiling file printing 3.","parse|= 3","rust"),
 ("rust_compile","src/lib.rs","Fix borrow error by returning owned String: fn first(v: Vec<String>) -> String. Include passing test for ['a'].","String","rust"),
 ("js_state","web/app.js","Fix stale state: const state={n:0}; function inc(){state.n=state.n+1}. Return complete JS calling inc twice and asserting n===2.","n===2|n !== 2","js"),
 ("js_state","web/app.js","Fix event listener so click increments once. Provide complete dependency-free JS simulation with one handler and assertion count===1.","count","js"),
 ("css_ui","web/style.css","Button text overflows. Return complete CSS using min-width:0 and overflow handling for .button-label.",r"min-width\s*:\s*0","css"),
 ("css_ui","web/style.css","Panel must scroll internally and stay in viewport. Return complete CSS for .panel using max-height and overflow-y:auto.",r"overflow-y\s*:\s*auto","css"),
 ("multi_file","src/lib.rs,README.md","Change both files. Return files array with complete src/lib.rs implementing add(a,b) plus two tests, and README.md documenting add.","fn add","multi_rust"),
 ("multi_file","web/app.js,web/view.js","Change both files. Return files array: web/app.js exports a model whose set increments; web/view.js imports it, renders the value, calls set, and throws unless render()===1.","render","multi_js"),
 ("refactor","src/lib.rs","Refactor duplicate clamps into fn clamp01(f64)->f64 using clamp. Include tests for -1, .5, 2.","clamp","rust"),
 ("refactor","web/app.js","Refactor repeated lowercase/trim into normalize(s), assert normalize(' X ')==='x'. Complete JS.","normalize","js"),
 ("tests","src/lib.rs","Add boundary tests and implementation for safe_div(a:i32,b:i32)->Option<i32>, None on zero.","None","rust"),
 ("tests","web/app.js","Implement sum(numbers) and executable assertions for empty array and [1,2,3].","reduce|sum","js"),
 ("terminal_debug","src/main.rs","A Rust test says expected 6 got 5 for sum [1,2,3]. Return corrected complete Rust with sum and passing test.","sum","rust"),
 ("diff_understand","src/lib.rs","Diff changed `>= limit` to `> limit`, breaking exact limit. Return complete Rust allowed(n,limit)->bool that allows n strictly below limit, with boundary test.","n < limit|< limit","rust"),
 ("repair","src/lib.rs","Repair UTF-8 panic from &s[..10]. Return complete Rust prefix(s,n)->String using chars and test with 'Grüße'.",r"chars\(\)","rust"),
 ("repair","web/app.js","Repair async error handling. Return complete JS async run() that catches rejected Promise and resolves 'fallback', then executable assertion.","catch|try","js"),
 ("tool_safety","src/lib.rs","Need inspect then patch; never delete. Return complete compiling Rust main printing 'safe'. Use tool fs.patch and file src/lib.rs.","safe","rust"),
]
SYSTEM='''You are Noki Code. Output exactly JSON. tool must be fs.patch. For one target use {"tool":"fs.patch","file":"exact target","code":"complete content"}. For multiple targets use {"tool":"fs.patch","files":[{"file":"exact target","code":"complete content"}]}. Paths must exactly match those named in TASK. Never output placeholders. No markdown. Never invent files or APIs.'''
def api(path,payload=None,timeout=900):
 data=None if payload is None else json.dumps(payload).encode();r=urllib.request.Request('http://127.0.0.1:11434'+path,data=data,headers={'Content-Type':'application/json'},method='GET' if data is None else 'POST');return json.load(urllib.request.urlopen(r,timeout=timeout))
def loaded():return [m.get('name',m.get('model','')) for m in api('/api/ps').get('models',[])]
def unload(m):
 api('/api/generate',{'model':m,'prompt':'','stream':False,'keep_alive':0})
 for _ in range(50):
  if not any(x.split(':latest')[0]==m.split(':latest')[0] for x in loaded()):return
  time.sleep(.1)
 raise RuntimeError('unload failed '+m)
def rss():
 try:return sum(int(x.split(None,1)[0]) for x in subprocess.check_output(['ps','-axo','rss=,command='],text=True).splitlines() if 'ollama runner' in x or '/Applications/Ollama.app/' in x)/1024
 except:return 0
def swap():
 try:
  s=subprocess.check_output(['/usr/sbin/sysctl','-n','vm.swapusage'],text=True);m=re.search(r'used = ([\d.]+)([MG])',s);return float(m.group(1))*(1024 if m.group(2)=='G' else 1) if m else 0
 except:return 0
def gen(model,prompt):
 peak=[rss()];stop=threading.Event()
 def poll():
  while not stop.wait(.05):peak[0]=max(peak[0],rss())
 threading.Thread(target=poll,daemon=True).start();t=time.monotonic()
 try:v=api('/api/generate',{'model':model,'prompt':SYSTEM+'\nTASK: '+prompt,'stream':False,'think':False,'keep_alive':'10m','format':'json','options':{'temperature':0,'seed':42,'num_ctx':4096,'num_predict':700}})
 finally:stop.set()
 return v,time.monotonic()-t,peak[0]
def parse(raw):
 try:return json.loads(raw)
 except:
  try:return json.loads(raw[raw.find('{'):raw.rfind('}')+1])
  except:return {}
def verify(kind,code,tmp,files=None):
 if kind=='multi_rust':
  files=files or {}; lib=files.get('src/lib.rs',''); readme=files.get('README.md','')
  comp,tests,log=verify('rust',lib,tmp);return comp,tests and bool(readme.strip()),log
 if kind=='multi_js':
  files=files or {}; web=Path(tmp)/'web';web.mkdir(exist_ok=True)
  for name in ('app.js','view.js'):(web/name).write_text(files.get('web/'+name,''))
  checks=[subprocess.run(['node','--check',str(web/name)],capture_output=True,text=True) for name in ('app.js','view.js')]
  comp=all(x.returncode==0 for x in checks);t=subprocess.run(['node',str(web/'view.js')],capture_output=True,text=True) if comp else checks[-1]
  return comp,t.returncode==0,t.stdout[-500:]+t.stderr[-500:]
 if kind=='rust':
  p=Path(tmp)/'x.rs';p.write_text(code);o=subprocess.run(['rustc','--test',str(p),'-o',str(Path(tmp)/'x')],capture_output=True,text=True);comp=o.returncode==0
  if not comp:return False,False,(o.stderr[-1000:])
  t=subprocess.run([str(Path(tmp)/'x')],capture_output=True,text=True);return True,t.returncode==0,t.stdout[-500:]+t.stderr[-500:]
 if kind=='js':
  p=Path(tmp)/'x.js';p.write_text(code);o=subprocess.run(['node','--check',str(p)],capture_output=True,text=True);comp=o.returncode==0
  t=subprocess.run(['node',str(p)],capture_output=True,text=True) if comp else o;return comp,t.returncode==0,t.stdout[-500:]+t.stderr[-500:]
 ok=code.count('{')==code.count('}') and code.count('(')==code.count(')');return ok,ok,''
def run(model):
 managed={'qwen2.5:3b-instruct-q5_K_M','qwen3.5:4b','qwen3.5:9b','qwen2.5:1.5b','qwen3:4b','qwen2.5-coder:7b','qwen2.5-coder:14b'}
 for x in loaded():
  if x in managed:unload(x)
 if loaded():raise RuntimeError('Foreign model loaded; benchmark paused without touching it: '+str(loaded()))
 swap0=swap()
 t=time.monotonic();api('/api/generate',{'model':model,'prompt':'','stream':False,'keep_alive':'10m'});cold=(time.monotonic()-t)*1000
 rows=[];peak=rss()
 with tempfile.TemporaryDirectory(prefix='noki-code-bench-') as tmp:
  for i,(cat,file,task,expected,kind) in enumerate(TASKS):
   v,wall,mem=gen(model,task);peak=max(peak,mem);obj=parse(v.get('response',''));tool=obj.get('tool','') if isinstance(obj,dict) else ''
   entries=obj.get('files',[]) if isinstance(obj,dict) else []; entries=entries if isinstance(entries,list) else []
   files={x.get('file',''):x.get('code','') for x in entries if isinstance(x,dict)}
   if isinstance(obj,dict) and obj.get('file'):files[obj.get('file','')]=obj.get('code','')
   wanted=file.split(',');code='\n'.join(files.values());wrong=len(set(files)^set(wanted))
   comp,tests,log=verify(kind,code,tmp,files) if code else (False,False,'no code');tool_ok=tool=='fs.patch';expected_ok=bool(re.search(expected,code,re.I|re.S));hallucinated=1 if re.search(r'fictional_api|TODO|unimplemented!',code,re.I) else 0;success=comp and tests and tool_ok and not wrong and expected_ok
   rows.append({'id':i+1,'category':cat,'task_success':success,'compile_success':comp,'tests_pass':tests,'tool_call_accuracy':tool_ok,'iterations_needed':1,'wrong_file_edits':wrong,'hallucinated_apis':hallucinated,'time_to_solution_ms':round(wall*1000),'tokens_per_second':round(v.get('eval_count',0)/(v.get('eval_duration',1)/1e9),2),'answer':obj,'log':log})
 loaded_ram=rss();unload(model);time.sleep(1);after=rss();n=len(rows)
 return {'model':model,'cases':n,'task_success':round(sum(r['task_success'] for r in rows)/n*100,1),'compile_success':round(sum(r['compile_success'] for r in rows)/n*100,1),'tests_pass':round(sum(r['tests_pass'] for r in rows)/n*100,1),'tool_call_accuracy':round(sum(r['tool_call_accuracy'] for r in rows)/n*100,1),'iterations_needed':1,'wrong_file_edits':sum(r['wrong_file_edits'] for r in rows),'hallucinated_apis':sum(r['hallucinated_apis'] for r in rows),'time_to_solution_ms':round(sum(r['time_to_solution_ms'] for r in rows)/n),'cold_load_ms':round(cold),'peak_ram_mb':round(peak),'loaded_ram_mb':round(loaded_ram),'ram_after_unload_mb':round(after),'swap_delta_mb':round(swap()-swap0,1),'rows':rows}
def main():
 p=argparse.ArgumentParser();p.add_argument('--models',nargs='+',default=['qwen2.5-coder:7b','qwen2.5-coder:14b']);p.add_argument('--out',default='.local/qa/code-model-ab.json');a=p.parse_args();res={'models':[run(m) for m in a.models]};Path(a.out).parent.mkdir(parents=True,exist_ok=True);Path(a.out).write_text(json.dumps(res,ensure_ascii=False,indent=2));print(json.dumps({'models':[{k:v for k,v in x.items() if k!='rows'} for x in res['models']]},indent=2))
if __name__=='__main__':main()
