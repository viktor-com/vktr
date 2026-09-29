#!/usr/bin/env python3
"""A local stand-in for the Viktor responses API on 127.0.0.1:8765: lists one model and streams a fixed markdown reply.
Used by measure.py so the benchmarks and recordings need no key and send nothing to Viktor.
"""
import json, time, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
MD = """# Brand check

Viktor **ships the change** and reports back. Here is what moved:

## What changed

- The wordmark now renders from pixel cells in `logo.rs`
- Themes use Peach, Lilac and Violet accents

### Code

```rust
fn main() {
    println!("hello from vktr");
}
```

> Quoted note: grounds follow the brand soft-black.

- [x] wordmark
- [ ] ship it

See [viktor.com/brand](https://viktor.com/brand) for the palette.
"""
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): sys.stderr.write(self.command+' '+self.path+'\n')
    def _json(self, obj, code=200):
        b=json.dumps(obj).encode(); self.send_response(code); self.send_header('content-type','application/json'); self.send_header('content-length',str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_GET(self): self._json({"object":"list","data":[{"id":"viktor","object":"model"}]})
    def do_POST(self):
        n=int(self.headers.get('content-length') or 0); body=self.rfile.read(n)
        sys.stderr.write(body[:300].decode(errors='replace')+'\n')
        self.send_response(200); self.send_header('content-type','text/event-stream'); self.end_headers()
        rid="resp_1"; mid="msg_1"
        seq=[0]
        def ev(t, d):
            d["type"]=t; d["sequence_number"]=seq[0]; seq[0]+=1; self.wfile.write(f"event: {t}\ndata: {json.dumps(d)}\n\n".encode()); self.wfile.flush()
        resp={"id":rid,"object":"response","created_at":1700000000,"status":"in_progress","model":"viktor","output":[],"parallel_tool_calls":False,"tool_choice":"auto","tools":[]}
        ev("response.created",{"response":resp})
        item={"id":mid,"type":"message","role":"assistant","status":"in_progress","content":[]}
        ev("response.output_item.added",{"output_index":0,"item":item})
        ev("response.content_part.added",{"item_id":mid,"output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}})
        for i in range(0,len(MD),40):
            ev("response.output_text.delta",{"item_id":mid,"output_index":0,"content_index":0,"delta":MD[i:i+40]}); time.sleep(0.02)
        ev("response.output_text.done",{"item_id":mid,"output_index":0,"content_index":0,"text":MD})
        done_item={"id":mid,"type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":MD,"annotations":[]}]}
        ev("response.output_item.done",{"output_index":0,"item":done_item})
        resp.update(status="completed",output=[done_item],usage={"input_tokens":10,"output_tokens":100,"total_tokens":110})
        ev("response.completed",{"response":resp})
ThreadingHTTPServer(('127.0.0.1',8765),H).serve_forever()
