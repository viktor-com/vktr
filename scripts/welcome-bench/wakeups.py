#!/usr/bin/env python3
"""Wakeups per second per thread of a running process over 10 s (voluntary + involuntary context switches). Usage: wakeups.py <pid>"""
import os,sys,time,glob
pid=sys.argv[1]
def snap():
    d={}
    for t in glob.glob(f'/proc/{pid}/task/*'):
        try:
            s=open(t+'/status').read(); name=open(t+'/comm').read().strip()
            v=int(s.split('voluntary_ctxt_switches:')[1].split()[0]); nv=int(s.split('nonvoluntary_ctxt_switches:')[1].split()[0])
            d[t.rsplit('/',1)[1]]=(name,v+nv)
        except Exception: pass
    return d
a=snap(); time.sleep(10); b=snap()
rows=sorted(((b[k][1]-a[k][1])/10,k,b[k][0]) for k in b if k in a)
tot=sum(r[0] for r in rows)
print(f"total wakeups/s {tot:.1f}; main thread {[r[0] for r in rows if r[1]==pid][0]:.1f}/s; busiest:", [(f'{r[2]}',round(r[0],1)) for r in rows[::-1][:4]])
