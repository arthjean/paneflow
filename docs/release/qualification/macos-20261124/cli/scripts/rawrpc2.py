import json, socket, sys
def session(path, hello, calls, timeout=4):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.settimeout(timeout); s.connect(path)
    f = s.makefile("rwb")
    out=[]
    seq = ([hello] if hello else []) + calls
    for i,(m,p) in enumerate(seq):
        f.write((json.dumps({"jsonrpc":"2.0","method":m,"params":p,"id":i+1})+"\n").encode()); f.flush()
        try:
            line=f.readline().decode().strip()
        except Exception as e:
            line="ERR %r"%e
        out.append((m,line[:400]))
        if not line: break
    return out
proc, path = sys.argv[1], sys.argv[2]
methods = sys.argv[3].split(",")
params = json.loads(sys.argv[4]) if len(sys.argv)>4 else {}
hello = None
if proc=="host": hello=("host.hello",{"client":"qual-probe","protocol":1})
if proc=="worker": hello=("worker.hello",{"client":"qual-probe","protocol":1})
for m in methods:
    for name,line in session(path, hello, [(m,params)]):
        print("==", proc, name, "->", line)
