#!/usr/bin/env python3
"""Viktor-compat mock: /v1/models, streaming /v1/chat/completions, /v1/responses and /v1/messages.

Smoke-tests a TUI against a custom endpoint without a real key. MOCK_MODE selects a
failure shape modelled on the Viktor compat API:
  ok             normal streamed reply (default)
  run_failed     HTTP 502 {"error": {"type": "server_error", "code": "run_failed"}}
  stream_error   chat: one data:{"error":...} frame then [DONE]; responses: response.failed;
                 messages: an `error` event
  empty          a stream that ends with no content at all
  unauthorized   HTTP 401 invalid key
  forbidden      HTTP 403 missing chat:completions scope
  tool_read / tool_bash / tool_edit / tool_write
                 first request: the model calls one caller tool (read_file, run_terminal_command,
                 search_replace, write); once the tool result comes back: a final text reply that
                 quotes the result, proving the round trip
The mode can also be set per request with header X-Mock-Mode. Requests are logged to stderr.
"""
import json
import re
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
MODEL = os.environ.get("MOCK_MODEL", "viktor")
TEXT = "MOCK_OK: hello from the mock endpoint."


THREADS = {}  # thread id -> number of input items received so far

TOOL_CALLS = {
    "tool_read": ("read_file", {"target_file": "main.py"}),
    "tool_bash": ("run_terminal_command", {"command": "echo TOOL_RAN > tool_ran.txt && cat tool_ran.txt", "description": "Write a marker file"}),
    "tool_bash_plain": ("run_terminal_command", {"command": "touch tool_ran_plain.txt", "description": "Create a marker file"}),
    "tool_edit": ("search_replace", {"file_path": "main.py", "old_string": "hello", "new_string": "goodbye"}),
    "tool_write": ("write", {"file_path": "notes.txt", "content": "written by the mock tool call\n"}),
    # vktr acp's editor tools (scripts/smoke-acp.py selects these with a [mock:<mode>] marker).
    "acp_read": ("editor_read_file", {"path": "hello.txt"}),
    "acp_write": ("editor_write_file", {"path": "made-by-viktor.txt", "content": "ACP_WRITE_OK\n"}),
    "acp_run": ("editor_run_command", {"command": "echo ACP_TOOL_RAN"}),
    "acp_mcp": ("mcp__mock__lookup", {"code": "Z9"}),
}
THREAD_MODES = {}  # thread id -> the mode its first request selected
CALLS = [0]  # tool calls issued so far, for unique call ids (editors key tool cards by id)


def _text_of(value):
    """Flatten a tool result payload (string, list of parts, dict) into text."""
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return " ".join(_text_of(v) for v in value)
    if isinstance(value, dict):
        for k in ("text", "output", "content"):
            if k in value:
                return _text_of(value[k])
        return json.dumps(value)
    return str(value)


def tool_result_in(req):
    """Return the most recent caller-tool result in the request history, or None."""
    found = None
    for item in req.get("input") or []:                      # responses
        if isinstance(item, dict) and item.get("type") == "function_call_output":
            found = _text_of(item.get("output"))
    for msg in req.get("messages") or []:                    # chat + anthropic
        if msg.get("role") == "tool":
            found = _text_of(msg.get("content"))
        if isinstance(msg.get("content"), list):
            for part in msg["content"]:
                if isinstance(part, dict) and part.get("type") == "tool_result":
                    found = _text_of(part.get("content"))
    return found


def reply_text(req, m):
    if m in TOOL_CALLS:
        result = tool_result_in(req) or ""
        return "MOCK_TOOL_OK: " + " ".join(result.split())[:120]
    return TEXT


def wants_tool_call(req, m):
    return m in TOOL_CALLS and tool_result_in(req) is None


def mode_for(handler):
    return handler.headers.get("X-Mock-Mode") or os.environ.get("MOCK_MODE", "ok")


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        pass

    def _json(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _sse_start(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

    def _sse(self, obj, event=None):
        if event:
            self.wfile.write(("event: " + event + "\n").encode())
        self.wfile.write(("data: " + json.dumps(obj) + "\n\n").encode())
        self.wfile.flush()

    def do_GET(self):
        sys.stderr.write("[mock] GET %s auth=%s\n" % (self.path, self.headers.get("Authorization", "")[:18]))
        sys.stderr.flush()
        if self.path.rstrip("/").endswith("/models"):
            m = mode_for(self)
            if m == "unauthorized":
                return self._json(401, {"error": {"message": "Invalid API key.", "type": "invalid_request_error", "code": "invalid_api_key"}})
            if m == "forbidden":
                return self._json(403, {"error": {"message": "This API key lacks the chat:completions scope.", "type": "permission_error", "code": "missing_scope"}})
            return self._json(200, {"object": "list", "data": [{"id": MODEL, "object": "model", "owned_by": "viktor"}]})
        return self._json(404, {"error": {"message": "not found"}})

    def do_POST(self):
        n = int(self.headers.get("Content-Length", "0"))
        req = json.loads(self.rfile.read(n) or b"{}")
        path = self.path.split("?", 1)[0].rstrip("/")  # Claude Code posts to /v1/messages?beta=true
        m = mode_for(self)
        dump = os.environ.get("MOCK_DUMP_DIR")
        if dump:
            os.makedirs(dump, exist_ok=True)
            with open(os.path.join(dump, "req-%d.json" % int(time.time() * 1000)), "w") as fh:
                json.dump({"path": path, "headers": dict(self.headers), "body": req}, fh, indent=1)
        sys.stderr.write("[mock] POST %s mode=%s auth=%s model=%s stream=%s prev=%s\n" % (
            path, m, (self.headers.get("Authorization") or ("x-api-key " + self.headers.get("x-api-key", "")))[:18], req.get("model"), req.get("stream"),
            req.get("previous_response_id")))
        sys.stderr.flush()
        if m == "unauthorized":
            return self._json(401, {"error": {"message": "Invalid API key.", "type": "invalid_request_error", "code": "invalid_api_key"}})
        if m == "forbidden":
            return self._json(403, {"error": {"message": "This API key lacks the chat:completions scope.", "type": "permission_error", "code": "missing_scope"}})
        if m == "run_failed":
            return self._json(502, {"error": {"message": "the model closed two consecutive streams without any content", "type": "server_error", "code": "run_failed", "param": None}})
        if path.endswith("/chat/completions"):
            return self.handle_chat(req, m)
        if path.endswith("/responses"):
            return self.handle_responses(req, m)
        if path.endswith("/messages"):
            return self.handle_messages(req, m)
        return self._json(404, {"error": {"message": "not found"}})

    # --- chat completions -------------------------------------------------
    def handle_chat(self, req, m):
        cid = "chatcmpl-mock"
        if not req.get("stream"):
            content = "" if m == "empty" else TEXT
            return self._json(200, {"id": cid, "object": "chat.completion", "created": int(time.time()), "model": MODEL,
                                    "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
                                    "usage": {"prompt_tokens": 10, "completion_tokens": 8, "total_tokens": 18}})
        self._sse_start()
        if m == "stream_error":
            self._sse({"error": {"message": "the model closed two consecutive streams without any content", "type": "server_error", "code": "run_failed", "param": None}})
            self.wfile.write(b"data: [DONE]\n\n"); self.wfile.flush()
            return
        if wants_tool_call(req, m):
            name, args = TOOL_CALLS[m]
            argstr = json.dumps(args)
            def chunk(delta, finish=None, usage=None):
                obj = {"id": cid, "object": "chat.completion.chunk", "created": int(time.time()), "model": MODEL,
                       "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
                if usage:
                    obj["usage"] = usage
                self._sse(obj)
            chunk({"role": "assistant", "content": None, "tool_calls": [{"index": 0, "id": "call_mock_1", "type": "function", "function": {"name": name, "arguments": ""}}]})
            half = len(argstr) // 2
            for frag in (argstr[:half], argstr[half:]):
                chunk({"tool_calls": [{"index": 0, "function": {"arguments": frag}}]})
            chunk({}, finish="tool_calls", usage={"prompt_tokens": 10, "completion_tokens": 8, "total_tokens": 18})
            self.wfile.write(b"data: [DONE]\n\n"); self.wfile.flush()
            return
        TEXT_OUT = reply_text(req, m)
        if m != "empty":
            for i, tok in enumerate(TEXT_OUT.split(" ")):
                self._sse({"id": cid, "object": "chat.completion.chunk", "created": int(time.time()), "model": MODEL,
                           "choices": [{"index": 0, "delta": {"role": "assistant", "content": ("" if i == 0 else " ") + tok}, "finish_reason": None}]})
                time.sleep(0.02)
        self._sse({"id": cid, "object": "chat.completion.chunk", "created": int(time.time()), "model": MODEL,
                   "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                   "usage": {"prompt_tokens": 10, "completion_tokens": 8, "total_tokens": 18}})
        self.wfile.write(b"data: [DONE]\n\n"); self.wfile.flush()

    # --- anthropic messages -------------------------------------------------
    def handle_messages(self, req, m):
        mid = "msg_mock"
        content = "" if m == "empty" else TEXT
        usage = {"input_tokens": 10, "output_tokens": 8}
        if not req.get("stream"):
            return self._json(200, {"id": mid, "type": "message", "role": "assistant", "model": MODEL,
                                    "content": ([{"type": "text", "text": content}] if content else []),
                                    "stop_reason": "end_turn", "stop_sequence": None, "usage": usage})
        self._sse_start()
        self._sse({"type": "message_start", "message": {"id": mid, "type": "message", "role": "assistant", "model": MODEL,
                                                         "content": [], "stop_reason": None, "stop_sequence": None,
                                                         "usage": {"input_tokens": 10, "output_tokens": 0}}}, event="message_start")
        if m == "stream_error":
            self._sse({"type": "error", "error": {"type": "api_error", "message": "the model closed two consecutive streams without any content"}}, event="error")
            return
        if wants_tool_call(req, m):
            name, args = TOOL_CALLS[m]
            argstr = json.dumps(args)
            self._sse({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_mock_1", "name": name, "input": {}}}, event="content_block_start")
            half = len(argstr) // 2
            for frag in (argstr[:half], argstr[half:]):
                self._sse({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": frag}}, event="content_block_delta")
            self._sse({"type": "content_block_stop", "index": 0}, event="content_block_stop")
            self._sse({"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": None}, "usage": {"output_tokens": 8}}, event="message_delta")
            self._sse({"type": "message_stop"}, event="message_stop")
            return
        TEXT_OUT = reply_text(req, m)
        if m != "empty":
            self._sse({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}, event="content_block_start")
            for i, tok in enumerate(TEXT_OUT.split(" ")):
                self._sse({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": ("" if i == 0 else " ") + tok}}, event="content_block_delta")
                time.sleep(0.02)
            self._sse({"type": "content_block_stop", "index": 0}, event="content_block_stop")
        self._sse({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 8}}, event="message_delta")
        self._sse({"type": "message_stop"}, event="message_stop")

    # --- responses --------------------------------------------------------
    def handle_responses(self, req, m):
        # Viktor semantics: the response id IS the durable thread id. No previous_response_id -> a fresh
        # thread; a known one -> the same thread (and the same id); an unknown one -> 404.
        prev = req.get("previous_response_id")
        # A "[mock:<mode>]" marker in the input picks the mode for the whole thread, so one mock
        # process can script different tool calls for different prompts (vktr acp cannot send
        # the X-Mock-Mode header).
        marker = re.search(r"\[mock:(\w+)\]", json.dumps(req.get("input")))
        if marker:
            m = marker.group(1)
        elif prev in THREAD_MODES:
            m = THREAD_MODES[prev]
        n_items = len(req["input"]) if isinstance(req.get("input"), list) else 1
        if prev:
            if prev not in THREADS:
                return self._json(404, {"error": {"message": "The requested previous_response_id was not found or is not accessible.",
                                                  "type": "invalid_request_error", "code": "previous_response_not_found", "param": "previous_response_id"}})
            rid = prev
        else:
            rid = "thread_mock_%d" % (len(THREADS) + 1)
        THREADS[rid] = THREADS.get(rid, 0) + n_items
        THREAD_MODES[rid] = m
        sys.stderr.write("[mock] responses thread=%s continued=%s items_in_request=%d items_in_thread=%d\n" % (rid, bool(prev), n_items, THREADS[rid]))
        sys.stderr.flush()
        if os.environ.get("MOCK_LOSE_THREADS"):
            THREADS.pop(rid, None)  # simulate a server that no longer has the thread on the next call
            THREADS["_lost_%d" % len(THREADS)] = 0  # keep ids unique
        TEXT_OUT = reply_text(req, m)
        content = "" if m == "empty" else TEXT_OUT
        msg = {"id": "msg_mock", "type": "message", "status": "completed", "role": "assistant",
               "content": [{"type": "output_text", "text": content, "annotations": []}]}
        base = {"id": rid, "object": "response", "created_at": int(time.time()), "model": MODEL,
                "output": [], "status": "in_progress", "error": None, "incomplete_details": None,
                "usage": None, "previous_response_id": req.get("previous_response_id")}
        if not req.get("stream"):
            done = dict(base, status="completed", output=[msg],
                        usage={"input_tokens": 10, "output_tokens": 8, "total_tokens": 18})
            return self._json(200, done)
        self._sse_start()
        seq = [0]
        def ev(name, obj):
            seq[0] += 1
            obj = dict(obj, type=name, sequence_number=seq[0])
            self._sse(obj, event=name)
        ev("response.created", {"response": base})
        ev("response.in_progress", {"response": base})
        if m == "stream_error":
            failed = dict(base, status="failed", error={"code": "server_error", "message": "the model closed two consecutive streams without any content"})
            ev("response.failed", {"response": failed})
            return
        if wants_tool_call(req, m):
            name, args = TOOL_CALLS[m]
            argstr = json.dumps(args)
            CALLS[0] += 1
            fc = {"id": "fc_mock_%d" % CALLS[0], "type": "function_call", "status": "in_progress", "call_id": "call_mock_%d" % CALLS[0], "name": name, "arguments": ""}
            ev("response.output_item.added", {"output_index": 0, "item": fc})
            half = len(argstr) // 2
            for frag in (argstr[:half], argstr[half:]):
                ev("response.function_call_arguments.delta", {"output_index": 0, "item_id": fc["id"], "delta": frag})
            ev("response.function_call_arguments.done", {"output_index": 0, "item_id": fc["id"], "arguments": argstr})
            fc_done = dict(fc, status="completed", arguments=argstr)
            ev("response.output_item.done", {"output_index": 0, "item": fc_done})
            ev("response.completed", {"response": dict(base, status="completed", output=[fc_done],
                                                        usage={"input_tokens": 10, "output_tokens": 8, "total_tokens": 18,
                                                               "input_tokens_details": {"cached_tokens": 0}, "output_tokens_details": {"reasoning_tokens": 0}})})
            return
        if m != "empty":
            ev("response.output_item.added", {"output_index": 0, "item": dict(msg, status="in_progress", content=[])})
            ev("response.content_part.added", {"output_index": 0, "item_id": "msg_mock", "content_index": 0,
                                               "part": {"type": "output_text", "text": "", "annotations": []}})
            for i, tok in enumerate(TEXT_OUT.split(" ")):
                ev("response.output_text.delta", {"output_index": 0, "item_id": "msg_mock", "content_index": 0,
                                                  "delta": ("" if i == 0 else " ") + tok})
                time.sleep(0.02)
            ev("response.output_text.done", {"output_index": 0, "item_id": "msg_mock", "content_index": 0, "text": TEXT_OUT})
            ev("response.content_part.done", {"output_index": 0, "item_id": "msg_mock", "content_index": 0,
                                              "part": {"type": "output_text", "text": TEXT_OUT, "annotations": []}})
            ev("response.output_item.done", {"output_index": 0, "item": msg})
        done = dict(base, status="completed", output=([] if m == "empty" else [msg]),
                    usage={"input_tokens": 10, "output_tokens": 8, "total_tokens": 18,
                           "input_tokens_details": {"cached_tokens": 0}, "output_tokens_details": {"reasoning_tokens": 0}})
        ev("response.completed", {"response": done})


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", PORT), H)
    # Printed once the port is bound, so the line means the mock is ready.
    sys.stderr.write("[mock] listening on 127.0.0.1:%d model=%s mode=%s\n" % (PORT, MODEL, os.environ.get("MOCK_MODE", "ok")))
    sys.stderr.flush()
    server.serve_forever()
