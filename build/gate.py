"""按需测试分组器：把「改了哪就跑哪」变成一条命令（对齐 CurSeen gate.py）。

用法：
  python build/gate.py list            # 列出全部分组
  python build/gate.py run config      # 跑 config 组（libtest 名称子串过滤）
  python build/gate.py run all         # 一次完整 release 测试（含集成）
  python build/gate.py --selftest      # mock cargo 的分组/空匹配自检

docstring 里的关键认知（CurSeen 原文，值得每次重读）：
  「测试本身只跑 1 秒，真正的开销是编译；按需检验省的是**信号**（改哪验哪、
   失败列表短），不是时间。真正省时间的是：rerun-if-changed 别指向不存在的
   路径 + 攒一批改动再编一次。」

组名 → 明确的模块命名空间（libtest 仍按大小写敏感子串过滤）。
每个过滤串在执行前用相同目标/过滤条件 --list，零测试或列举失败即失败。
all 不组合分组，而是不带 --lib/过滤串完整运行一次，避免漏集成与重复测试。
TEMP/TMP 使用工作区 .temp；RUSTUP_TOOLCHAIN 保留调用方设置，无设置时兜底。
"""
import os
import subprocess
import sys
from pathlib import Path

MAIN = Path(__file__).resolve().parent.parent

GROUPS = {
    "config":   "core::config::",
    "paths":    "core::paths::",
    "gallery":  "core::gallery::",
    "decoders": "decoders::",
    "pixels":   "core::pixels::",
    "geometry": "core::geometry::",
    "viewer":   "core::viewer::",
    "decode":   "core::decode::",
    "ext":      ["core::ext::", "extensions::"],
    "abi":      "core::abi::",
    "ui":       "ui::",
}


def run_one(name: str, pat) -> bool:
    if isinstance(pat, list):
        # libtest 只吃单个过滤串；多模式组逐个跑（组小，编译是共享的）。
        ok = True
        for p in pat:
            ok &= run_one(f"{name}/{p}", p)
        return ok
    print(f"--- gate: {name} ({pat}) ---")
    # 本机 rustup 没配 default toolchain（K31）：cargo 会直接报
    # "could not choose a version of cargo to run"。脚本自己兜底注入，
    # 让 DOCS_MANIFEST 登记的命令在任何干净环境里都能跑。
    env = dict(os.environ)
    env.setdefault("RUSTUP_TOOLCHAIN", "stable-x86_64-pc-windows-msvc")
    # 测试夹具的落点已由 `core::paths::test_scratch_root()` 钉死在工作区 `.temp/rupix-test/`
    # （2026-10-29：以前只靠这里改 TEMP，IDE 裸跑 `cargo test` 就漏进系统 TEMP，本机一次就
    # 积过 369 个目录）。这里继续把 TEMP/TMP 指过来是给 link.exe 一类的中间文件用的——
    # 它必须是**不带 `..` 的绝对路径**，否则 `link.exe` 按自己的 CWD 再解一次会 LNK1104。
    env["TEMP"] = env["TMP"] = str(MAIN.parent / ".temp")
    command = ["cargo", "test", "--release"]
    if pat is not None:
        command += ["--lib", pat]
    command += ["--", "--test-threads=1"]
    listing = subprocess.run(
        command + ["--list"], cwd=MAIN, env=env,
        stdout=subprocess.PIPE, encoding="utf-8", errors="replace",
    )
    if listing.returncode != 0:
        print(f"[ERROR] {name}: cargo --list failed")
        return False
    count = sum(line.endswith(": test") for line in listing.stdout.splitlines())
    if count == 0:
        print(f"[ERROR] {name}: no matching tests; refusing an empty gate")
        return False
    print(f"matched tests: {count}")
    return subprocess.run(command, cwd=MAIN, env=env).returncode == 0


def selftest() -> int:
    """只 mock cargo 进程边界，不编译、不争 cargo 锁。"""
    import contextlib
    import io
    import unittest
    from unittest.mock import patch

    class GateTests(unittest.TestCase):
        def invoke(self, group, outputs):
            with patch.object(sys, "argv", ["gate.py", "run", group]), \
                    patch.object(subprocess, "run", side_effect=outputs) as run, \
                    contextlib.redirect_stdout(io.StringIO()):
                code = main()
            return code, run.call_args_list

        def test_all_includes_integration_once(self):
            code, calls = self.invoke("all", [
                subprocess.CompletedProcess([], 0, "suite::case: test\n"),
                subprocess.CompletedProcess([], 0),
            ])
            self.assertEqual(code, 0)
            command = ["cargo", "test", "--release", "--", "--test-threads=1"]
            self.assertEqual([c.args[0] for c in calls],
                             [command + ["--list"], command])

        def test_groups_list_same_target_and_filter(self):
            expected = {
                "config": ["core::config::"], "paths": ["core::paths::"],
                "gallery": ["core::gallery::"], "decoders": ["decoders::"],
                "pixels": ["core::pixels::"], "geometry": ["core::geometry::"],
                "viewer": ["core::viewer::"], "decode": ["core::decode::"],
                "ext": ["core::ext::", "extensions::"], "abi": ["core::abi::"],
                "ui": ["ui::"],
            }
            for group, filters in expected.items():
                with self.subTest(group=group):
                    outputs = [subprocess.CompletedProcess([], 0, "case: test\n"),
                               subprocess.CompletedProcess([], 0)] * len(filters)
                    code, calls = self.invoke(group, outputs)
                    self.assertEqual(code, 0)
                    commands = []
                    for pat in filters:
                        cmd = ["cargo", "test", "--release", "--lib", pat,
                               "--", "--test-threads=1"]
                        commands.extend([cmd + ["--list"], cmd])
                    self.assertEqual([c.args[0] for c in calls], commands)

        def test_empty_list_fails_without_execution(self):
            for listing in ["", "0 tests, 0 benchmarks\n", "case: benchmark\n"]:
                with self.subTest(listing=listing):
                    code, calls = self.invoke("ui", [
                        subprocess.CompletedProcess([], 0, listing)])
                    self.assertEqual(code, 1)
                    self.assertEqual(len(calls), 1)

        def test_list_and_test_failure_propagate(self):
            for outputs in [
                [subprocess.CompletedProcess([], 7, "case: test\n")],
                [subprocess.CompletedProcess([], 0, "case: test\n"),
                 subprocess.CompletedProcess([], 9)],
            ]:
                code, calls = self.invoke("config", outputs)
                self.assertEqual(code, 1)
                self.assertEqual(len(calls), len(outputs))

        def test_environment(self):
            with patch.dict(os.environ, {"RUSTUP_TOOLCHAIN": "custom"}, clear=True):
                code, calls = self.invoke("config", [
                    subprocess.CompletedProcess([], 0, "case: test\n"),
                    subprocess.CompletedProcess([], 0)])
            self.assertEqual(code, 0)
            for call in calls:
                self.assertEqual(call.kwargs["cwd"], MAIN)
                self.assertEqual(call.kwargs["env"]["RUSTUP_TOOLCHAIN"], "custom")
                for key in ["TEMP", "TMP"]:
                    self.assertEqual(call.kwargs["env"].get(key), str(MAIN.parent / ".temp"))

    result = unittest.TextTestRunner().run(unittest.defaultTestLoader.loadTestsFromTestCase(GateTests))
    return 0 if result.wasSuccessful() else 1


def main() -> int:
    args = sys.argv[1:]
    if args == ["--selftest"]:
        return selftest()
    if not args or args[0] == "list":
        for k, v in GROUPS.items():
            print(f"  {k:10s} -> {v}")
        return 0
    if args[0] == "run" and len(args) >= 2:
        which = args[1]
        if which == "all":
            return 0 if run_one("all", None) else 1
        if which not in GROUPS:
            print(f"unknown group: {which} (try: list)")
            return 2
        return 0 if run_one(which, GROUPS[which]) else 1
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main())
