#!/usr/bin/env python3
"""Describe already-downloaded voice files for Grafium; no network or model conversion."""
import argparse
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
from urllib.parse import urlsplit


def prepare(args):
    root = Path(args.folder).expanduser()
    if root.is_symlink() or not root.is_dir():
        raise ValueError("Choose a regular folder containing one extracted voice.")
    root = root.resolve()
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,96}", args.id):
        raise ValueError("Voice ID must contain 1-96 ASCII letters, digits, underscores or hyphens.")
    if not re.fullmatch(r"[A-Za-z][A-Za-z0-9]{0,7}(?:-[A-Za-z0-9]{1,8})*", args.language) or len(args.language) > 63:
        raise ValueError("Use a BCP-47 language tag, such as en-US, not en_US.")
    if not args.name.strip() or len(args.name.encode()) > 256 or not args.license.strip() or len(args.license.encode()) > 256:
        raise ValueError("Name and reviewed license must be nonempty and at most 256 bytes.")
    url = urlsplit(args.license_url)
    host = url.hostname or ""
    if (url.scheme != "https" or not host or "." not in host or url.username or url.password
            or url.query or url.fragment or host.endswith((".local", ".localhost"))):
        raise ValueError("License URL must be public HTTPS without credentials, query or fragment.")
    try:
        ipaddress.ip_address(host)
    except ValueError:
        pass
    else:
        raise ValueError("Use the publisher's hostname, not an IP address.")

    models = sorted(root.glob("*.onnx"))
    if len(models) != 1:
        raise ValueError("The folder must contain exactly one .onnx model at its top level.")
    files = [(models[0], "model"), (root / args.license_file, "license")]
    rate = args.sample_rate
    if args.runtime == "piper-onnx-v1":
        config_path = models[0].with_name(models[0].name + ".json")
        if config_path.is_symlink() or not config_path.is_file() or config_path.stat().st_size > 16 * 1024 * 1024:
            raise ValueError("Keep the matching <model>.onnx.json beside the model.")
        config = json.loads(config_path.read_text(encoding="utf-8"))
        if not isinstance(config, dict) or not isinstance(config.get("audio"), dict) or not isinstance(config.get("language", {}), dict):
            raise ValueError("Piper configuration must contain audio metadata and valid language metadata.")
        actual_rate = config.get("audio", {}).get("sample_rate")
        if rate is not None and rate != actual_rate:
            raise ValueError("Sample rate does not match the Piper configuration.")
        rate = actual_rate
        model_language = config.get("language", {}).get("code")
        if model_language and model_language.replace("_", "-").lower() != args.language.lower():
            raise ValueError("Language does not match the Piper configuration.")
        files.append((config_path, "config"))
    else:
        files.append((root / "tokens.txt", "tokens"))
        if (root / "lexicon.txt").exists():
            files.append((root / "lexicon.txt", "lexicon"))
        data = root / "espeak-ng-data"
        if data.is_symlink():
            raise ValueError("Phonemizer data must not be a symbolic link.")
        if data.exists():
            for path in sorted(data.rglob("*")):
                if path.is_symlink():
                    raise ValueError("Phonemizer data must not contain symbolic links.")
                if path.is_file():
                    files.append((path, "espeak"))
    if type(rate) is not int or not 8000 <= rate <= 48000:
        raise ValueError("Supply the model's actual sample rate (8000-48000); Android needs --sample-rate.")
    if len(files) > 2048:
        raise ValueError("The package exceeds 2048 files.")
    artifacts = []
    total = 0
    paths = set()
    for path, role in files:
        relative = path.relative_to(root).as_posix()
        if (not relative or len(relative.encode()) > 240 or any(c in relative for c in "\\:\0")
                or any(part in ("", ".", "..") for part in relative.split("/"))
                or relative == "manifest.json" or relative in paths):
            raise ValueError("Voice files must have unique safe relative paths.")
        paths.add(relative)
        if path.is_symlink() or any(parent.is_symlink() for parent in path.parents if parent != root) or not path.is_file():
            raise ValueError(f"Missing regular file (symlinks are not supported): {relative}")
        before = path.stat()
        limit = (512 if role == "model" else 16) * 1024 * 1024
        if not 1 <= before.st_size <= limit:
            raise ValueError(f"Empty or oversized file: {relative}")
        total += before.st_size
        if total > 768 * 1024 * 1024:
            raise ValueError("The package exceeds 768 MiB.")
        digest = hashlib.sha256()
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(chunk)
        after = path.stat()
        if (before.st_size, before.st_mtime_ns, before.st_ino) != (after.st_size, after.st_mtime_ns, after.st_ino):
            raise ValueError(f"File changed while hashing; retry with unchanged files: {relative}")
        artifacts.append({"path": relative, "role": role, "bytes": before.st_size, "sha256": digest.hexdigest(), "url": None})
    manifest = {"schema_version": 1, "id": args.id, "name": args.name, "language": args.language,
                "runtime": args.runtime, "license": args.license, "license_url": args.license_url,
                "sample_rate": rate, "artifacts": artifacts}
    encoded = json.dumps(manifest, indent=2, ensure_ascii=False) + "\n"
    if len(encoded.encode()) > 512 * 1024:
        raise ValueError("Manifest exceeds 512 KiB.")
    destination = root / "manifest.json"
    with destination.open("x", encoding="utf-8") as output:
        output.write(encoded)
        output.flush()
        os.fsync(output.fileno())
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folder")
    parser.add_argument("--runtime", required=True, choices=["piper-onnx-v1", "sherpa-vits-v1"])
    for name in ["id", "name", "language", "license", "license-url"]:
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--license-file", default="MODEL_CARD", help="Reviewed local license/model card; default MODEL_CARD")
    parser.add_argument("--sample-rate", type=int)
    args = parser.parse_args()
    try:
        destination = prepare(args)
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"Could not prepare voice package: {error}\n")
    print(f"Created {destination}. Originals were not changed. Import this package in Grafium.")
    print("Hashes describe your local files; they do not prove publisher identity, license rights or engine compatibility.")


if __name__ == "__main__":
    main()
