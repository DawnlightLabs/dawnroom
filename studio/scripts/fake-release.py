"""A fake GitHub release on localhost, to test the install scripts, `dawnroom update` and Studio's updater.

Puts the given files in <dir>/good and <dir>/bad (same files, but one SHA256SUMS line is wrong in
bad), writes SHA256SUMS and release.json (shaped like GitHub's releases API), then serves <dir>:
    python studio/scripts/fake-release.py <dir> <version> <port> file1 file2 ...
    DAWNROOM_RELEASES=http://127.0.0.1:<port>/good/release.json   (install scripts, `dawnroom update`)
When an installer has its updater signature beside it (`<file>.sig`, from a signed Tauri build), it
also writes the updater's manifest, as the real release's `latest.json` asset:
    DAWNROOM_UPDATE_JSON=https://127.0.0.1:<port>/good/download/latest.json   (Studio)
The updater only takes https: with FAKE_TLS_CERT and FAKE_TLS_KEY set (a self-signed pair), it
serves https; Studio accepts a self-signed certificate only from a loopback test endpoint.
"""
import datetime, functools, hashlib, http.server, json, os, pathlib, shutil, ssl, sys

root, version, port, files = pathlib.Path(sys.argv[1]), sys.argv[2], int(sys.argv[3]), sys.argv[4:]
tls = os.environ.get("FAKE_TLS_CERT") and os.environ.get("FAKE_TLS_KEY")
scheme = "https" if tls else "http"
# The updater's platform keys, by installer type.
PLATFORMS = {"-setup.exe": "windows-x86_64", ".AppImage": "linux-x86_64", ".deb": "linux-x86_64-deb"}

for kind in ("good", "bad"):
    d = root / kind
    d.mkdir(parents=True, exist_ok=True)
    sums = []
    for f in map(pathlib.Path, files):
        shutil.copyfile(f, d / f.name)
        h = hashlib.sha256((d / f.name).read_bytes()).hexdigest()
        if kind == "bad" and not sums:
            h = "0" * 64  # the first file's checksum doesn't match
        sums.append(f"{h}  {f.name}")
    (d / "SHA256SUMS").write_text("\n".join(sums) + "\n", newline="\n")
    names = [pathlib.Path(f).name for f in files] + ["SHA256SUMS"]
    base = f"{scheme}://127.0.0.1:{port}/{kind}"
    (d / "release.json").write_text(json.dumps({
        "tag_name": f"v{version}",
        "html_url": f"https://github.com/DawnlightLabs/dawnroom/releases/tag/v{version}",
        "assets": [{"name": n, "size": (d / n).stat().st_size, "browser_download_url": f"{base}/{n}"} for n in names],
    }, indent=1))
    platforms = {}
    for f in map(pathlib.Path, files):
        sig = pathlib.Path(str(f) + ".sig")
        key = next((v for k, v in PLATFORMS.items() if f.name.endswith(k)), None)
        if key and sig.exists():
            platforms[key] = {"signature": sig.read_text().strip(), "url": f"{base}/{f.name}"}
    if platforms:
        (d / "download").mkdir(exist_ok=True)
        (d / "download" / "latest.json").write_text(json.dumps({
            "version": version,
            "notes": f"Dawnroom {version} (test release)",
            "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "platforms": platforms,
        }, indent=1))


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        super().end_headers()


server = http.server.ThreadingHTTPServer(("127.0.0.1", port), functools.partial(Handler, directory=str(root)))
if tls:
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(os.environ["FAKE_TLS_CERT"], os.environ["FAKE_TLS_KEY"])
    server.socket = ctx.wrap_socket(server.socket, server_side=True)
print(f"serving {root} on {scheme}://127.0.0.1:{port}", flush=True)
server.serve_forever()
