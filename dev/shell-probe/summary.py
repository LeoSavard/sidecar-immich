#!/usr/bin/env python3
# One line per probe result in testdata/cache/shell-probe/: what WebGPU it had, and what reached the window.
import json, pathlib, sys
d = pathlib.Path(__file__).resolve().parent.parent.parent / "testdata/cache/shell-probe"
for f in sorted(d.glob("*.json")) if len(sys.argv) < 2 else [d / f"{n}.json" for n in sys.argv[1:]]:
    if not f.exists():
        log = f.with_suffix(".log")
        tail = log.read_text().strip().splitlines()[-4:] if log.exists() else ["(no log)"]
        print(f"== {f.stem}  NO RESULT; log tail:", *("   " + t[:200] for t in tail), sep="\n")
        continue
    r = json.loads(f.read_text())
    p = r.get("page") or {}
    a = p.get("adapter") or {}
    st = r.get("gpuFeatureStatus", {})
    print(f"== {f.stem}  electron {r['electron']} chrome {r['chrome']}  switches {r['switches'] or '-'}")
    print(f"   status: webgpu={st.get('webgpu')} vulkan={st.get('vulkan')} compositing={st.get('gpu_compositing')}  vulkan-version={r.get('gpuInfo', {}).get('vulkan')}")
    if r.get("parity"):
        print(f"   parity: {r['parity']}")
        continue
    print(f"   navigator.gpu={p.get('hasGpu')}  adapter={a.get('vendor')}/{a.get('architecture')} {a.get('description') or ''} fallback={a.get('fallback')}")
    if p.get("adapter"):
        print(f"   compute={p.get('compute')}  preferred={p.get('preferredFormat')}  f16={'shader-f16' in p.get('features', [])}  float32-filterable={'float32-filterable' in p.get('features', [])}")
        print(f"   canvas: {p.get('canvas')}")
    print(f"   presented: {r.get('presented')}  (want {p.get('paint', {}).get('want')})  error: {p.get('error')}  timedOut: {r.get('timedOut')}")
    errs = [c for c in r.get("console", []) if c.split(':')[0] in ('2', '3', 'warning', 'error')]
    if errs:
        print(f"   console: {errs[-3:]}")
