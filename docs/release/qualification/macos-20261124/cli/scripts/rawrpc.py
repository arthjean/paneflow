import json, socket, sys
def call(path, method, params, timeout=3):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.settimeout(timeout)
    s.connect(path)
    s.sendall((json.dumps({"jsonrpc":"2.0","method":method,"params":params,"id":1})+"\n").encode())
    buf=b""
    try:
        while b"\n" not in buf:
            c=s.recv(65536)
            if not c: break
            buf+=c
    except Exception as e:
        return "ERR %r (partial %r)" % (e, buf[:300])
    return buf.decode().strip()
if __name__ == "__main__":
    path, method = sys.argv[1], sys.argv[2]
    params = json.loads(sys.argv[3]) if len(sys.argv) > 3 else {}
    print(call(path, method, params))
