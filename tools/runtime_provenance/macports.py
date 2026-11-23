"""Resolve active MacPorts receipts, original Portfiles and checksum-verified distfiles."""
# SPDX-License-Identifier: GPL-3.0-or-later

import hashlib
import os
import re
import shutil
import sqlite3
import subprocess
import tarfile
import urllib.parse
from pathlib import Path

from .common import Notices, archive_kind, command, digest, download, notice_name, safe_name


def prefix_for(files):
    origin = Path(next(iter(files.values()))["source"])
    for prefix in origin.parents:
        if (prefix / "var/macports/registry/registry.db").is_file():
            return prefix
    raise RuntimeError(f"No MacPorts registry above {origin}")


def records(prefix, files):
    registry = prefix / "var/macports/registry"
    db = sqlite3.connect((registry / "registry.db").as_uri() + "?mode=ro", uri=True)
    db.row_factory = sqlite3.Row
    try:
        groups = {}
        for name, item in files.items():
            source = str(Path(item["source"]))
            q = "SELECT id FROM files WHERE active=1 AND actual_path=?"
            row = db.execute(q, (source,)).fetchone()
            if not row:
                row = db.execute(q, (str(Path(source).resolve()),)).fetchone()
            if not row:
                raise RuntimeError(f"Unowned MacPorts runtime file: {source}")
            groups.setdefault(row["id"], []).append(name)
        packages = []
        for port_id, paths in groups.items():
            row = dict(db.execute("SELECT * FROM ports WHERE id=?", (port_id,)).fetchone())
            row["outputs"] = sorted(paths)
            row["groups"] = [dict(pg) for pg in db.execute("SELECT name,version,size,sha256 FROM portgroups WHERE id=?", (port_id,))]
            row["noticePaths"] = [Path(p[0]) for p in db.execute(
                "SELECT actual_path FROM files WHERE id=? AND active=1", (port_id,)) if p[0] and
                ("/share/licenses/" in p[0] or ("/share/doc/" in p[0] and notice_name(p[0])))]
            packages.append(row)
        return sorted(packages, key=lambda p: p["name"])
    finally:
        db.close()


def recipe_location(port, row, registry):
    current = Path(command([port, "-q", "dir", row["name"]]).splitlines()[-1])
    receipt = registry / "portfiles" / f"{row['name']}-{row['version']}_{row['revision']}" / row["portfile"] / "Portfile"
    portfile = current / "Portfile"
    if not receipt.is_file() or not portfile.is_file() or digest(receipt) != digest(portfile):
        raise RuntimeError(f"Active recipe for {row['name']} differs from its installed Portfile; use the matching MacPorts tree")
    tree = next((parent for parent in current.parents if (parent / "_resources/port1.0/group").is_dir()), None)
    if not tree:
        raise RuntimeError(f"Cannot locate the MacPorts ports tree for {portfile}")
    for group in row["groups"]:
        used = tree / "_resources/port1.0/group" / f"{group['name']}-{group['version']}.tcl"
        if not used.is_file() or digest(used) != group["sha256"] or used.stat().st_size != group["size"]:
            raise RuntimeError(f"Installed PortGroup for {row['name']} differs from {used}")
    return tree, portfile


def archive_recipes(tree, target):
    def included(member):
        parts = Path(member.name).parts
        if any(p in {".git", ".svn", "work", "PortIndex", "PortIndex.quick"}
               or p.startswith("PortIndex_") for p in parts):
            return None
        return member
    target.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(target, "w:gz", compresslevel=1, dereference=False) as archive:
        archive.add(tree, arcname="ports", filter=included)


def distfiles(text):
    files = []
    for line in text.splitlines():
        header = re.match(r"^\[([^\]]+)\]\s+(.+)$", line)
        if header:
            name, path = header.groups()
            files.append({"name": safe_name(name), "cache": Path(path), "checksums": {}, "urls": []})
        elif files:
            value = line.strip()
            match = re.fullmatch(r"(sha256|sha512|sha1|rmd160|md5|size):\s*(\S+)", value, re.I)
            if match:
                files[-1]["checksums"][match[1].lower()] = match[2].lower()
            elif value.startswith("https://"):
                files[-1]["urls"].append(value)
    return files


def matches(path, sums):
    if not path.is_file() or not sums:
        return False
    verified = False
    for kind, expected in sums.items():
        if kind == "size":
            if path.stat().st_size != int(expected):
                return False
            continue
        try:
            computed = digest(path, "ripemd160" if kind == "rmd160" else kind)
        except ValueError:
            if kind == "rmd160" and "sha256" in sums:
                continue
            raise
        if computed.lower() != expected:
            return False
        verified = True
    return verified


def retrieve(item, destination):
    sums = item["checksums"]
    if matches(item["cache"], sums):
        shutil.copyfile(item["cache"], destination)
        return "MacPorts distfile cache"
    # Prefer MacPorts' mirrors, which retain source versions after many upstreams move.
    urls = sorted(dict.fromkeys(item["urls"]), key=lambda u: "distfiles.macports.org" not in u)
    for url in urls:
        try:
            download(url, destination)
            if matches(destination, sums):
                return url
        except OSError:
            pass
        destination.unlink(missing_ok=True)
    raise RuntimeError(f"No verified source for {item['name']} using the installed Portfile checksums {sums}")


def run_port(port, *args):
    env = {**os.environ, "LANG":"C", "LC_ALL":"C", "COLUMNS":"300"}
    cmd = [str(port), *args]
    run = subprocess.run(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if run.returncode and any(s in run.stderr.lower() for s in ("permission", "privilege", "root", "read-only")):
        return command(["sudo", "-n", *cmd], env=env)
    if run.returncode:
        raise RuntimeError(f"{' '.join(cmd)} failed: {run.stderr}")
    return run.stdout.strip()


def collect(files, notices_root, output):
    prefix = prefix_for(files)
    port = prefix / "bin/port"
    registry = prefix / "var/macports/registry"
    recipes, packages = {}, {}
    for row in records(prefix, files):
        name = row["name"]
        version = f"{row['version']}_{row['revision']}"
        key = safe_name(name + "-" + version)
        print(f"Collecting MacPorts sources for {key}", flush=True)
        tree, recipe = recipe_location(port, row, registry)
        if tree not in recipes:
            recipes[tree] = output / "recipes" / f"macports-ports-{len(recipes)+1}.tar.gz"
            archive_recipes(tree, recipes[tree])
        active = re.findall(r"\+[A-Za-z0-9_.]+", row["variants"] or "")
        deselected = re.findall(r"-[A-Za-z0-9_.]+", row.get("requested_variants") or "")
        listing = run_port(port, "distfiles", name, *active, *deselected)
        inputs = distfiles(listing)
        if not inputs:
            raise RuntimeError(f"The installed {name} Portfile lists no source distfiles. Supply the exact VCS checkout or generated-source recipe for this port.")
        notices = Notices(notices_root / "texts")
        for original in row["noticePaths"]:
            notices.installed(original)
        for original in recipe.parent.rglob("*"):
            if original.is_file() and not original.is_symlink() and notice_name(original.relative_to(recipe.parent).as_posix()):
                notices.installed(original)
        sources = []
        for item in inputs:
            destination = output / "sources" / key / item["name"]
            destination.parent.mkdir(parents=True, exist_ok=True)
            url = retrieve(item, destination)
            notices.archive(destination)
            sources.append({"file": destination.relative_to(output).as_posix(), "sha256": digest(destination),
                            "obtainedFrom": url, "upstreamURLs": item["urls"], "portChecksums": item["checksums"]})
        licenses = run_port(port, "-q", "info", "--license", name)
        packages[key] = {"binaryPackages": [{"name":name, "version":version, "epoch":row["epoch"],
                         "variants":row["variants"], "requestedVariants":row.get("requested_variants"), "licenses":licenses}],
                         "files":row["outputs"], "sources":sources, "notices":notices.references,
                         "recipeTree":recipes[tree].relative_to(output).as_posix(),
                         "portfile":recipe.relative_to(tree).as_posix(), "portfileSha256":digest(recipe),
                         "portGroups":row["groups"], "packagingTool":command([port,"version"])}
    return packages
