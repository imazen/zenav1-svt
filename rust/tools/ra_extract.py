#!/usr/bin/env python3
"""Drive rust-analyzer's "Extract into function" assist from the command line.

    tools/ra_extract.py <file.rs> <first_line> <last_line> <new_name> [...]

Several `<first> <last> <name>` triples may follow the file; they are applied
BOTTOM-UP (highest line first) in one rust-analyzer session, so the line
numbers you pass are all from the file as it is now. Lines are 1-based and
inclusive and must cover whole statements.

rust-analyzer does the part that is error-prone by hand: it finds every free
variable of the selection, decides which are passed by value, `&` or
`&mut`, and which values flow back out, and writes the call site. This
script only asks for the assist, renames the generated `fun_name` to
`<new_name>`, and applies the edit. It is how the 7,600-line
`encode_frame_impl` is cut into stages. Review the result: the assist is
correct about data flow, but it can produce long parameter lists that are
worth bundling by hand afterwards.

Needs `rustup component add rust-analyzer`. Indexing the workspace takes a
minute or two; run under run-heavy.
"""
import json
import os
import pathlib
import subprocess
import sys
import threading
import time

RUST = pathlib.Path(__file__).resolve().parents[1]


class Lsp:
    def __init__(self):
        self.p = subprocess.Popen(
            ["rust-analyzer"], cwd=RUST, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=open(os.path.expanduser("~/tmp/ra_extract.stderr.log"), "w"),
        )
        self.id = 0
        self.responses = {}
        self.progress_done = threading.Event()
        self.active = set()
        self.cv = threading.Condition()
        threading.Thread(target=self._reader, daemon=True).start()

    def _send(self, msg):
        body = json.dumps(msg).encode()
        self.p.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
        self.p.stdin.flush()

    def _reader(self):
        f = self.p.stdout
        while True:
            headers = {}
            while True:
                line = f.readline()
                if not line:
                    return
                line = line.decode().strip()
                if not line:
                    break
                k, v = line.split(":", 1)
                headers[k.lower()] = v.strip()
            msg = json.loads(f.read(int(headers["content-length"])))
            if "id" in msg and "method" in msg:  # server -> client request
                result = None
                if msg["method"] == "workspace/configuration":
                    result = [None for _ in msg["params"]["items"]]
                self._send({"jsonrpc": "2.0", "id": msg["id"], "result": result})
                continue
            if msg.get("method") == "$/progress":
                v = msg["params"]["value"]
                tok = str(msg["params"]["token"])
                if v.get("kind") == "begin":
                    self.active.add(tok)
                elif v.get("kind") == "end":
                    self.active.discard(tok)
                    if not self.active:
                        self.progress_done.set()
                continue
            if "id" in msg:
                with self.cv:
                    self.responses[msg["id"]] = msg
                    self.cv.notify_all()

    def request(self, method, params, timeout=600):
        self.id += 1
        rid = self.id
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        with self.cv:
            ok = self.cv.wait_for(lambda: rid in self.responses, timeout)
        if not ok:
            sys.exit(f"ra_extract: {method} timed out")
        msg = self.responses.pop(rid)
        if "error" in msg:
            sys.exit(f"ra_extract: {method} failed: {msg['error']}")
        return msg["result"]

    def notify(self, method, params):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})


def apply_edits(text, edits):
    lines = text.split("\n")
    offs = [0]
    for l in lines:
        offs.append(offs[-1] + len(l) + 1)

    def off(pos):
        # LSP positions are UTF-16; the sources are ASCII in the ranges we edit
        # except comments, so convert through the line's UTF-16 encoding.
        line = lines[pos["line"]] if pos["line"] < len(lines) else ""
        u16 = line.encode("utf-16-le")[: pos["character"] * 2].decode("utf-16-le")
        return offs[pos["line"]] + len(u16)

    for e in sorted(edits, key=lambda e: (e["range"]["start"]["line"], e["range"]["start"]["character"]), reverse=True):
        a, b = off(e["range"]["start"]), off(e["range"]["end"])
        text = text[:a] + e["newText"] + text[b:]
    return text


def main():
    if len(sys.argv) < 5 or (len(sys.argv) - 2) % 3:
        sys.exit(__doc__)
    path = pathlib.Path(sys.argv[1]).resolve()
    jobs = [(int(sys.argv[i]), int(sys.argv[i + 1]), sys.argv[i + 2]) for i in range(2, len(sys.argv), 3)]
    jobs.sort(reverse=True)
    uri = path.as_uri()
    lsp = Lsp()
    lsp.request("initialize", {
        "processId": os.getpid(), "rootUri": RUST.as_uri(),
        "capabilities": {"window": {"workDoneProgress": True},
                         "textDocument": {"codeAction": {"resolveSupport": {"properties": ["edit"]},
                                                         "codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["refactor", "refactor.extract"]}}}}},
        "initializationOptions": {"checkOnSave": False, "cachePriming": {"enable": False}},
    })
    lsp.notify("initialized", {})
    text = path.read_text()
    version = 1
    lsp.notify("textDocument/didOpen", {"textDocument": {"uri": uri, "languageId": "rust", "version": version, "text": text}})
    t0 = time.time()
    # Wait for indexing: progress quiet for 20 s, and at least one round seen.
    lsp.progress_done.wait(900)
    while True:
        time.sleep(5)
        if not lsp.active:
            time.sleep(15)
            if not lsp.active:
                break
    print(f"ra_extract: indexed in {time.time() - t0:.0f}s", flush=True)

    for first, last, name in jobs:
        lines = text.split("\n")
        end_char = len(lines[last - 1].encode("utf-16-le")) // 2
        start_char = len(lines[first - 1]) - len(lines[first - 1].lstrip())
        rng = {"start": {"line": first - 1, "character": start_char}, "end": {"line": last - 1, "character": end_char}}
        actions = lsp.request("textDocument/codeAction", {
            "textDocument": {"uri": uri}, "range": rng,
            "context": {"diagnostics": [], "only": ["refactor.extract"]},
        })
        act = next((a for a in actions or [] if a.get("title") == "Extract into function"), None)
        if act is None:
            sys.exit(f"ra_extract: no 'Extract into function' for {first}-{last}: {[a.get('title') for a in actions or []]}")
        if "edit" not in act:
            act = lsp.request("codeAction/resolve", act)
        edit = act["edit"]
        edits = []
        for dc in edit.get("documentChanges", []):
            if dc["textDocument"]["uri"] != uri:
                sys.exit("ra_extract: edit touches another file")
            edits += dc["edits"]
        for u, es in edit.get("changes", {}).items():
            if u != uri:
                sys.exit("ra_extract: edit touches another file")
            edits += es
        for e in edits:
            e["newText"] = e["newText"].replace("$0", "").replace("fun_name", name)
        text = apply_edits(text, edits)
        version += 1
        lsp.notify("textDocument/didChange", {"textDocument": {"uri": uri, "version": version},
                                               "contentChanges": [{"text": text}]})
        time.sleep(2)
        print(f"ra_extract: {first}-{last} -> {name}", flush=True)
    path.write_text(text)
    lsp.p.terminate()


if __name__ == "__main__":
    main()
