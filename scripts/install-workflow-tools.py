#!/usr/bin/env python3
"""Install exact workflow lint tools into the ignored repository cache."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

ACTIONLINT_REVISION = "fd33e9f582a01a02885c93ae3775e1190cf63653"
ACTIONLINT_SHA256 = "475da14da4ed169c493d4f988f2cd6e4f15434752635ad809e5613cc3cde6fcc"
SHELLCHECK_VERSION = "0.11.0"
SHELLCHECK_SHA256 = {
    ("Darwin", "arm64"): "339b930feb1ea764467013cc1f72d09cd6b869ebf1013296ba9055ab2ffbd26f",
    ("Darwin", "x86_64"): "c2c15e08df0e8fbc374c335b230a7ee958c313fa5714817a59aa59f1aa594f51",
    ("Linux", "aarch64"): "68a8133197a50beb8803f8d42f9908d1af1c5540d4bb05fdfca8c1fa47decefc",
    ("Linux", "x86_64"): "b7af85e41cc99489dcc21d66c6d5f3685138f06d34651e6d34b42ec6d54fe6f6",
}


def output(binary, *args):
    try:
        return subprocess.run([str(binary), *args], text=True, capture_output=True, timeout=10, check=True).stdout
    except (OSError, subprocess.SubprocessError) as error:
        raise RuntimeError(f"cannot verify {binary.name}: {error}") from error


def verify_actionlint(binary):
    identity = output(binary, "-version")
    if ACTIONLINT_REVISION not in identity:
        raise RuntimeError(f"actionlint identity mismatch: expected {ACTIONLINT_REVISION}, got {identity.strip()!r}")
    return identity.strip()


def verify_shellcheck(binary):
    if not binary.is_file():
        raise RuntimeError(f"ShellCheck {SHELLCHECK_VERSION} is missing at {binary}")
    identity = output(binary, "--version")
    if f"version: {SHELLCHECK_VERSION}" not in identity:
        raise RuntimeError(f"ShellCheck identity mismatch: expected {SHELLCHECK_VERSION}, got {identity.strip()!r}")
    return identity.strip()


def write_receipt(root, actionlint, shellcheck):
    receipt = root / "receipt.json"
    receipt.write_text(json.dumps({
        "actionlint_revision": ACTIONLINT_REVISION,
        "actionlint_identity": verify_actionlint(actionlint),
        "shellcheck_version": SHELLCHECK_VERSION,
        "shellcheck_identity": verify_shellcheck(shellcheck),
    }, indent=2, sort_keys=True) + "\n")
    return receipt


def download(url, destination, expected):
    with urllib.request.urlopen(url, timeout=60) as response, destination.open("wb") as target:
        shutil.copyfileobj(response, target)
    actual = hashlib.sha256(destination.read_bytes()).hexdigest()
    if actual != expected:
        raise RuntimeError(f"checksum mismatch for {url}: expected {expected}, got {actual}")


def safe_extract(archive, destination):
    with tarfile.open(archive, "r:gz") as source:
        for member in source.getmembers():
            if member.islnk() or member.issym() or ".." in Path(member.name).parts:
                raise RuntimeError(f"unsafe archive member: {member.name}")
        source.extractall(destination, filter="data")


def install(root):
    bindir = root / "bin"
    bindir.mkdir(parents=True, exist_ok=True)
    actionlint = bindir / "actionlint"
    shellcheck = bindir / "shellcheck"
    try:
        write_receipt(root, actionlint, shellcheck)
        return bindir
    except RuntimeError:
        pass
    key = (platform.system(), platform.machine())
    if key not in SHELLCHECK_SHA256:
        raise RuntimeError(f"unsupported workflow-tool platform: {key[0]} {key[1]}")
    os_name = key[0].lower()
    arch = "aarch64" if key[1] == "arm64" else key[1]
    with tempfile.TemporaryDirectory(prefix="workflow-tools-") as temporary:
        temporary = Path(temporary)
        action_archive = temporary / "actionlint.tar.gz"
        download(f"https://github.com/rhysd/actionlint/archive/{ACTIONLINT_REVISION}.tar.gz", action_archive, ACTIONLINT_SHA256)
        action_source = temporary / "actionlint"
        action_source.mkdir()
        safe_extract(action_archive, action_source)
        source_root = next(action_source.iterdir())
        env = os.environ.copy()
        env["GOMODCACHE"] = str(root / "gomodcache")
        subprocess.run([
            "go", "build", "-trimpath",
            "-ldflags", f"-s -w -X github.com/rhysd/actionlint.version={ACTIONLINT_REVISION} -X github.com/rhysd/actionlint.installedFrom=Headstate-verified-source",
            "-o", str(actionlint), "./cmd/actionlint",
        ], cwd=source_root, env=env, check=True)
        shell_archive = temporary / "shellcheck.tar.gz"
        shell_name = f"shellcheck-v{SHELLCHECK_VERSION}.{os_name}.{arch}.tar.gz"
        download(f"https://github.com/koalaman/shellcheck/releases/download/v{SHELLCHECK_VERSION}/{shell_name}", shell_archive, SHELLCHECK_SHA256[key])
        shell_source = temporary / "shellcheck"
        shell_source.mkdir()
        safe_extract(shell_archive, shell_source)
        shutil.copy2(next(shell_source.glob("*/shellcheck")), shellcheck)
        shellcheck.chmod(0o755)
    write_receipt(root, actionlint, shellcheck)
    return bindir


def main():
    repository = Path(__file__).resolve().parent.parent
    root = Path(os.environ.get("WORKFLOW_TOOLS_DIR", repository / ".tools" / "workflow"))
    print(install(root))


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Workflow tool installation failed: {error}", file=sys.stderr)
        raise SystemExit(1)
