#!/usr/bin/env python3
"""Build and smoke-test a self-contained, licensed native Krit archive."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import time
import zipfile

ROOT = Path(__file__).resolve().parents[2]
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
)
NOTICE_PREFIXES = ("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE")
SOURCE_SUFFIXES = {".rs", ".c", ".h", ".cpp", ".py", ".js"}


def command(arguments, cwd=ROOT):
    result = subprocess.run(
        arguments, cwd=cwd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    if result.returncode:
        raise SystemExit(
            f"Command failed ({result.returncode}): {arguments}\n"
            f"{result.stdout}{result.stderr}"
        )
    return result.stdout


def sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def dependencies(metadata, root_id):
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    pending = [root_id]
    visited = set()
    while pending:
        package_id = pending.pop()
        if package_id in visited:
            continue
        visited.add(package_id)
        for dependency in nodes[package_id]["deps"]:
            if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"]):
                pending.append(dependency["pkg"])
    return sorted(
        (package for package in metadata["packages"] if package["id"] in visited),
        key=lambda package: (package["name"], package["version"]),
    )


def vcs_revision(package):
    path = Path(package["manifest_path"]).parent / ".cargo_vcs_info.json"
    if not path.is_file():
        return None
    return json.loads(path.read_text(encoding="utf-8"))["git"]["sha1"]


def bundle_notices(metadata, root_id, payload):
    packages = dependencies(metadata, root_id)
    wasmtime = next(package for package in packages if package["name"] == "wasmtime")
    wasmtime_root = Path(wasmtime["manifest_path"]).parent.resolve()
    wasmtime_revision = vcs_revision(wasmtime)
    third_party = []
    for package in packages:
        if package["id"] in metadata["workspace_members"]:
            continue
        source_root = Path(package["manifest_path"]).parent.resolve()
        notices = {
            path
            for path in source_root.rglob("*")
            if path.is_file()
            and path.suffix.lower() not in SOURCE_SUFFIXES
            and (
                path.name.upper().startswith(NOTICE_PREFIXES)
                or "LICENSES" in (part.upper() for part in path.relative_to(source_root).parts)
            )
        }
        if package["license_file"]:
            notices.add(source_root / package["license_file"])
        notice_source = f"{package['name']} {package['version']}"
        # Several Wasmtime workspace crates omit their shared project LICENSE
        # from the crate archive. Reuse it only for the exact same source revision.
        if (
            not notices
            and package["name"].startswith(("cranelift-", "pulley-", "wasmtime-"))
            and package["license"] == wasmtime["license"]
            and wasmtime_revision is not None
            and vcs_revision(package) == wasmtime_revision
        ):
            source_root = wasmtime_root
            notices = {wasmtime_root / "LICENSE"}
            notice_source = f"wasmtime {wasmtime['version']} ({wasmtime_revision})"
        if not notices:
            raise SystemExit(
                f"No license notices found for {package['name']} {package['version']}"
            )
        destination = payload / "licenses" / f"{package['name']}-{package['version']}"
        destination.mkdir(parents=True)
        copied = []
        for index, path in enumerate(sorted(notices)):
            if not path.resolve(strict=True).is_relative_to(source_root):
                raise SystemExit(f"Dependency notice escapes its package: {package['name']}")
            name = f"{index:04d}-{path.name}"
            shutil.copyfile(path, destination / name)
            copied.append((destination / name).relative_to(payload).as_posix())
        third_party.append(
            {
                "name": package["name"],
                "version": package["version"],
                "license": package["license"],
                "noticeSource": notice_source,
                "notices": copied,
            }
        )
    (payload / "THIRD-PARTY-LICENSES.json").write_text(
        json.dumps(third_party, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def make_archive(payload, output, epoch, windows):
    files = sorted(path for path in payload.rglob("*") if path.is_file())
    if windows:
        with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for path in files:
                info = zipfile.ZipInfo(
                    path.relative_to(payload.parent).as_posix(),
                    time.gmtime(max(epoch, 315532800))[:6],
                )
                info.create_system = 3
                info.external_attr = 0o100644 << 16
                archive.writestr(
                    info, path.read_bytes(), compress_type=zipfile.ZIP_DEFLATED
                )
    else:
        with output.open("wb") as destination:
            with gzip.GzipFile(fileobj=destination, mode="wb", filename="", mtime=epoch) as compressed:
                with tarfile.open(fileobj=compressed, mode="w") as archive:
                    for path in files:
                        info = archive.gettarinfo(
                            str(path), arcname=path.relative_to(payload.parent).as_posix()
                        )
                        info.uid = info.gid = 0
                        info.uname = info.gname = ""
                        info.mtime = epoch
                        info.mode = 0o755 if path.name == "krit" else 0o644
                        with path.open("rb") as source:
                            archive.addfile(info, source)


def smoke_archive(archive_path, package_name, version, windows):
    with tempfile.TemporaryDirectory(prefix="krit-download-") as directory:
        if windows:
            with zipfile.ZipFile(archive_path) as archive:
                archive.extractall(directory)
        else:
            with tarfile.open(archive_path) as archive:
                archive.extractall(directory, filter="data")
        extracted = Path(directory) / package_name
        executable = str(extracted / ("krit.exe" if windows else "krit"))
        expected = (
            (["--version"], f"Krit {version}"),
            (["run", "examples/factorial.krit"], "720"),
            (["run", "examples/lists.krit"], "[10, 20, 12]\n42"),
        )
        for arguments, output in expected:
            if command([executable, *arguments], extracted).strip() != output:
                raise SystemExit(f"Extracted package produced incorrect output: {arguments}")
        command([executable, "package", "check"], extracted)
        command(
            [executable, "fmt", "--check", "examples/factorial.krit", "examples/lists.krit"],
            extracted,
        )
        command([executable, "build", "--output", "factorial.wasm"], extracted)
        permissions = json.loads(
            command(
                [executable, "permissions", "--json", "--artifact", "factorial.wasm"],
                extracted,
            )
        )
        if (
            permissions["localGrantStatus"] != "allowed"
            or permissions["denied"]
            or permissions["imports"] != ["krit:runtime/stdout@0.2.0"]
        ):
            raise SystemExit("Extracted package reported incorrect artifact permissions")
        if command([executable, "sandbox", "--artifact", "factorial.wasm"], extracted).strip() != "720":
            raise SystemExit("Extracted package could not execute its Wasm artifact")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "target" / "dist")
    arguments = parser.parse_args()
    windows = arguments.target.endswith("-windows-msvc")
    executable_name = "krit.exe" if windows else "krit"
    binary = arguments.binary or ROOT / "target" / arguments.target / "release" / executable_name
    if not binary.is_file():
        raise SystemExit(f"Build the release binary before packaging: {binary}")
    metadata = json.loads(
        command(
            [
                "cargo", "metadata", "--format-version", "1", "--locked",
                "--filter-platform", arguments.target,
            ]
        )
    )
    cli = next(package for package in metadata["packages"] if package["name"] == "krit-cli")
    version = cli["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise SystemExit("Release packaging requires a stable MAJOR.MINOR.PATCH version")
    revision = command(["git", "rev-parse", "HEAD"]).strip()
    epoch = int(command(["git", "show", "-s", "--format=%ct", "HEAD"]).strip())
    name = f"krit-{version}-{arguments.target}"
    arguments.output.mkdir(parents=True, exist_ok=True)
    archive_path = arguments.output.resolve() / f"{name}{'.zip' if windows else '.tar.gz'}"
    if archive_path.exists():
        raise SystemExit(f"Refusing to overwrite an existing package: {archive_path}")
    with tempfile.TemporaryDirectory(prefix="krit-package-") as directory:
        payload = Path(directory) / name
        payload.mkdir()
        shutil.copyfile(binary, payload / executable_name)
        for filename in ("LICENSE", "NOTICE", "README.md", "krit.pkg"):
            shutil.copyfile(ROOT / filename, payload / filename)
        (payload / "examples").mkdir()
        for filename in ("factorial.krit", "lists.krit"):
            shutil.copyfile(ROOT / "examples" / filename, payload / "examples" / filename)
        bundle_notices(metadata, cli["id"], payload)
        (payload / "BUILD.json").write_text(
            json.dumps(
                {
                    "schema": 1,
                    "name": "krit",
                    "version": version,
                    "target": arguments.target,
                    "sourceCommit": revision,
                    "binarySha256": sha256(binary),
                },
                indent=2,
                sort_keys=True,
            ) + "\n",
            encoding="utf-8",
        )
        make_archive(payload, archive_path, epoch, windows)
    smoke_archive(archive_path, name, version, windows)
    checksum = sha256(archive_path)
    archive_path.with_name(archive_path.name + ".sha256").write_text(
        f"{checksum}  {archive_path.name}\n", encoding="ascii"
    )
    if output_path := os.environ.get("GITHUB_OUTPUT"):
        with open(output_path, "a", encoding="utf-8") as output:
            output.write(f"archive={archive_path}\n")
    print(f"Packaged {archive_path.name} ({checksum})")


if __name__ == "__main__":
    main()
