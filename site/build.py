#!/usr/bin/env python3
"""Builds the website in site/ into a folder for GitHub Pages.

The download links, the version and the release's highlights come from the
newest published release on GitHub and are written into the page, so it
needs nothing from GitHub when it's viewed. .github/workflows/pages.yml runs
this when a release is published; locally:

    python3 site/build.py                    # into target/site
    python3 -m http.server -d target/site    # http://localhost:8000

GH_TOKEN (or GITHUB_TOKEN) is used for the API call when set; without it
GitHub allows 60 calls an hour. --release-json takes the release from a file
instead (the API's JSON for one release).
"""

import argparse
import html
import json
import os
import re
import shutil
import sys
import urllib.request
from datetime import datetime
from pathlib import Path

REPO = "ortic/gotthard"
ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"

# What each file of a release is, from its name. The page's links follow
# this order; the first is the file the big button offers.
KINDS = [
    (r"gotthard_[^_]+_x64-setup\.exe", "Installer (.exe)"),
    (r"gotthard_[^_]+_x64_en-US\.msi", "Installer (.msi)"),
    (r"gotthard-v[^-]+-windows-x86_64\.zip", "Portable (.zip)"),
]
CHECKSUMS = "SHA256SUMS.txt"


def fetch_latest():
    req = urllib.request.Request(
        f"https://api.github.com/repos/{REPO}/releases/latest",
        headers={"Accept": "application/vnd.github+json", "User-Agent": "gotthard-site-build"},
    )
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    with urllib.request.urlopen(req, timeout=30) as res:
        return json.load(res)


def classify(assets):
    """[(label, asset)] in the page's order, and the problems found."""
    found = []
    problems = []
    used = {CHECKSUMS}
    for pattern, label in KINDS:
        regex = re.compile(pattern)
        match = [a for a in assets if regex.fullmatch(a["name"])]
        if len(match) != 1:
            problems.append(f"{len(match)} files match {regex.pattern}")
            continue
        used.add(match[0]["name"])
        found.append((label, match[0]))
    if not any(a["name"] == CHECKSUMS for a in assets):
        problems.append(f"no {CHECKSUMS}")
    for a in assets:
        if a["name"] not in used:
            # A new kind of package: not an error, but the page doesn't offer it.
            print(f"::warning::{a['name']} isn't on the download page; add it to KINDS in site/build.py")
    return found, problems


def mb(size):
    return f"{size / 1048576:.0f} MB"


def esc(text):
    return html.escape(text, quote=True)


def links(files):
    rows = []
    for label, asset in files:
        rows.append(
            f'<li><a href="{esc(asset["browser_download_url"])}">{esc(label)}</a>'
            f'<span class="size">{mb(asset["size"])}</span></li>'
        )
    return "\n".join(rows)


def inline_md(text):
    """The Markdown of a release note line: code, bold, italics, links, #123."""
    parts = re.split(r"(`[^`]+`)", text)
    out = []
    for part in parts:
        if part.startswith("`") and part.endswith("`") and len(part) > 1:
            out.append(f"<code>{esc(part[1:-1])}</code>")
            continue
        part = esc(part)
        part = re.sub(r"\[([^\]]+)\]\((https?://[^)\s]+)\)", r'<a href="\2">\1</a>', part)
        part = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", part)
        part = re.sub(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])", r"<em>\1</em>", part)
        part = re.sub(r"(?<![\w/&])#(\d+)\b", rf'<a href="https://github.com/{REPO}/issues/\1">#\1</a>', part)
        out.append(part)
    return "".join(out)


def highlights(body):
    """The bullets under "## Highlights" in the release notes, as <li>s."""
    match = re.search(r"^## Highlights\s*$(.*?)(?=^## |\Z)", body or "", re.M | re.S)
    if not match:
        return ""
    items = []
    for line in match.group(1).splitlines():
        if re.match(r"^[-*] ", line):
            items.append(line[2:].strip())
        elif line.startswith("  ") and items:
            items[-1] += " " + line.strip()
    return "\n".join(f"<li>{inline_md(item)}</li>" for item in items)


def render(release, base_url):
    assets = release["assets"]
    found, problems = classify(assets)
    if problems:
        sys.exit(
            f"The page can't offer every package of {release['tag_name']}:\n  "
            + "\n  ".join(problems)
            + "\nFix the release or KINDS in site/build.py."
        )

    tag = release["tag_name"]
    version = tag.removeprefix("v")
    date = datetime.fromisoformat(release["published_at"].replace("Z", "+00:00"))
    checksums = next(a for a in assets if a["name"] == CHECKSUMS)

    values = {
        "version": esc(version),
        "date": f"{date:%B} {date.day}, {date.year}",
        "date_iso": f"{date:%Y-%m-%d}",
        "release_url": esc(release["html_url"]),
        "checksums_url": esc(checksums["browser_download_url"]),
        "base_url": esc(base_url.rstrip("/") + "/"),
        "highlights": highlights(release.get("body")),
    }
    values["links_windows"] = links(found)
    values["main_windows"] = esc(found[0][1]["browser_download_url"])
    values["size_windows"] = mb(found[0][1]["size"])

    page = (SITE / "index.html").read_text(encoding="utf-8")
    if not values["highlights"]:
        page = re.sub(r"<!-- highlights -->.*?<!-- /highlights -->", "", page, flags=re.S)

    def fill(match):
        key = match.group(1)
        if key not in values:
            sys.exit(f"site/index.html: no value for {{{{{key}}}}}")
        return values[key]

    return re.sub(r"\{\{\s*(\w+)\s*\}\}", fill, page)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", type=Path, default=ROOT / "target" / "site")
    parser.add_argument("--base-url", default="https://ortic.github.io/gotthard/",
                        help="where the site is published, for its canonical and preview links")
    parser.add_argument("--release-json", type=Path, help="the release's API JSON instead of asking GitHub")
    args = parser.parse_args()

    release = json.loads(args.release_json.read_text()) if args.release_json else fetch_latest()
    page = render(release, args.base_url)

    out = args.out
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    (out / "index.html").write_text(page, encoding="utf-8")
    for name in ("style.css", "site.js"):
        shutil.copy2(SITE / name, out / name)
    shots = out / "screenshots"
    shots.mkdir()
    for name in sorted(set(re.findall(r"screenshots/([\w-]+\.png)", page))):
        shutil.copy2(ROOT / "docs" / "screenshots" / name, shots / name)
    icons = out / "icons"
    icons.mkdir()
    for name in ("gotthard-32.png", "gotthard-128.png", "gotthard-256.png"):
        shutil.copy2(ROOT / "assets" / "icon" / name, icons / name)
    print(f"{release['tag_name']}: {out / 'index.html'}")


if __name__ == "__main__":
    main()
