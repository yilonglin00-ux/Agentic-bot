#!/usr/bin/env python3
"""Explicit setup only; never called by Noki or its build/start scripts."""
import argparse, hashlib, json, pathlib, urllib.request
p = argparse.ArgumentParser(description='Ein kleines offizielles Noki-Modell einmalig herunterladen.')
p.add_argument('--model', choices=['0.5b', '1.5b'], required=True)
a = p.parse_args()
repo = 'Qwen/Qwen2.5-' + a.model.upper() + '-Instruct-GGUF'
name = 'qwen2.5-' + a.model + '-instruct-q4_k_m.gguf'
root = pathlib.Path(__file__).resolve().parent
with urllib.request.urlopen('https://huggingface.co/api/models/' + repo + '?blobs=true', timeout=30) as r:
    info = json.load(r)
entry = next(f for f in info['siblings'] if f['rfilename'] == name)
lfs = entry['lfs']; expected = lfs['sha256']; size = lfs['size']
if size > 1_300_000_000: raise SystemExit('Modell überschreitet die konservative Größenbegrenzung.')
url = 'https://huggingface.co/' + repo + '/resolve/' + info['sha'] + '/' + name
print(name, size, 'bytes, revision', info['sha'], flush=True)
partial = root / (name + '.partial'); digest = hashlib.sha256(); total = 0
with urllib.request.urlopen(url, timeout=120) as r, partial.open('wb') as out:
    while chunk := r.read(1024*1024):
        total += len(chunk)
        if total > size: raise SystemExit('Download unerwartet groß.')
        out.write(chunk); digest.update(chunk)
if total != size or digest.hexdigest() != expected: raise SystemExit('SHA-256/Größe stimmt nicht: Datei bleibt .partial.')
partial.replace(root / name)
(root / (name + '.json')).write_text(json.dumps({'repo': repo, 'revision':info['sha'], 'filename':name, 'bytes':size, 'sha256':expected, 'license':'Apache-2.0'}, indent=2)+'\n')
print('PASS SHA-256:', expected, flush=True)
