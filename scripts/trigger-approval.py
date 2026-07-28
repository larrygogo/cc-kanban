# 一次性触发工具：向运行中的 Meowo broker 发送一条模拟授权请求（v2 协议，
# 与 meowo-reporter 的 request_approval 同一线路），用来现场验证：
# 后台会话授权 → 不切窗不抢焦点 + 侧边栏徽标 + 新审批卡。
# 用法: python scripts/trigger-approval.py <session_id> [--tool Bash] [--desc "..."]
import json
import socket
import struct
import sys
import time
from pathlib import Path

def main() -> int:
    args = sys.argv[1:]
    if not args:
        print("usage: trigger-approval.py <session_id> [--tool NAME] [--desc TEXT]")
        return 2
    session_id = int(args[0])
    tool = args[args.index("--tool") + 1] if "--tool" in args else "Bash"
    desc = args[args.index("--desc") + 1] if "--desc" in args else "运行测试命令(模拟授权请求,直接点拒绝即可)"

    discovery_path = Path.home() / ".meowo" / "approval-broker.json"
    discovery = json.loads(discovery_path.read_text(encoding="utf-8"))
    endpoint = discovery["endpoint"]
    token = discovery["token"]
    host, port = endpoint.rsplit(":", 1)

    envelope = {
        "version": 2,
        "request": {
            "kind": "approval",
            "token": token,
            "request": {
                "sessionId": session_id,
                "requestId": f"manual-trigger-{int(time.time() * 1000)}",
                "provider": "claude",
                "toolName": tool,
                "description": desc,
                "input": json.dumps({"command": "echo 这是一条模拟的授权请求"}, ensure_ascii=False, indent=2),
                "permissionSuggestions": [
                    {
                        "type": "addRules",
                        "behavior": "allow",
                        "destination": "session",
                        "rules": [{"toolName": tool, "ruleContent": "echo *"}],
                    }
                ],
            },
        },
    }
    payload = json.dumps(envelope, ensure_ascii=False).encode("utf-8")

    with socket.create_connection((host, int(port)), timeout=3) as conn:
        conn.sendall(b"MWO2" + struct.pack(">I", len(payload)) + payload)
        conn.settimeout(310)
        decision = b""
        while True:
            chunk = conn.recv(4096)
            if not chunk:
                break
            decision += chunk
    print(f"decision: {decision.decode('utf-8', 'replace').strip() or '(连接被关闭,无决定)'}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
