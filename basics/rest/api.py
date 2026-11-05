"""Zero-dependency REST demo: python3 api.py, then curl localhost:8000/users"""
import json
from http.server import BaseHTTPRequestHandler, HTTPServer

USERS = [{"id": 1, "name": "Ada"}]


class API(BaseHTTPRequestHandler):
    def _send(self, code, obj):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/users":
            self._send(200, USERS)
        else:
            self._send(404, {"error": "not found"})

    def do_POST(self):
        if self.path != "/echo":
            return self._send(404, {"error": "not found"})
        data = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
        self._send(201, {"you_sent": data})

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    print("listening on http://localhost:8000")
    HTTPServer(("localhost", 8000), API).serve_forever()
