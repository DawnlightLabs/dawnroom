"""Writes registry/<id>.toml (schema 2) from registry/sources/models.toml and live sources:

- Hugging Face API: canonical repo id, pinned revision (commit sha), file size, LFS sha256,
  the original repo's licence id and gated flag. Every file is then checked with a HEAD request
  (it exists and its size matches); nothing big is downloaded. Small non-LFS files (tokenizers)
  have no published sha256, so those (< 64 MB) are downloaded once and hashed.
- sd-server's built-in recipe at the pinned sd.cpp commit: steps, guidance, sampler, scheduler,
  flow shift, distilled guidance, custom sigmas, default size.

Existing `measured` blocks are kept (schema 1 measurements move to the variant whose weights
they were measured with), and a file's existing revision is kept while its sha256 is unchanged,
so re-runs don't churn. Prints what changed. Sequential requests, backs off on 429, no token.

    python engine/scripts/gen_models.py            # all models
    python engine/scripts/gen_models.py sd15 sdxl  # some
"""

import hashlib
import json
import pathlib
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request

SDCPP_COMMIT = "465adf389f252e401257f44efb0ea49da8e4e80b"  # release master-1086-465adf3
BACKEND = "sd-cpp@master-1086-465adf3"
HF = "https://huggingface.co"
UA = {"User-Agent": "dawnroom-model-generator"}
SERVER_FLAGS = ["--flash-attn", "on", "--vae-tiling"]
SMALL_FILE = 64 * 1024 * 1024

CATEGORY = {
    "diffusion_model": "diffusion_models",
    "uncond_diffusion_model": "diffusion_models",
    "checkpoint": "checkpoints",
    "llm": "text_encoders",
    "llm_vision": "text_encoders",
    "clip_l": "text_encoders",
    "clip_g": "text_encoders",
    "t5xxl": "text_encoders",
    "vae": "vae",
    "tokenizer": "tokenizers",
}
ROLE_ORDER = list(CATEGORY)

# HF licence ids we can label without reading the card. Anything else ("other") needs a
# hand-read `licence` in the sources file.
KNOWN_LICENCES = {
    "apache-2.0": ("Apache-2.0", "Apache License 2.0", True, False, None),
    "mit": ("MIT", "MIT License", True, False, None),
    "creativeml-openrail-m": ("LicenseRef-CreativeML-OpenRAIL-M", "CreativeML Open RAIL-M", True, True,
                              "OpenRAIL-M: commercial use allowed, with use-based restrictions."),
    "openrail++": ("LicenseRef-CreativeML-OpenRAIL-PlusPlus-M", "CreativeML Open RAIL++-M", True, True,
                   "OpenRAIL++: commercial use allowed, with use-based restrictions."),
}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *a, **k):
        return None


def request(url, method="GET", data=None, headers=None, follow=True, tries=8):
    """One polite request: backs off on 429/5xx (Retry-After honoured)."""
    opener = urllib.request.build_opener() if follow else urllib.request.build_opener(NoRedirect)
    delay = 2
    for _ in range(tries):
        req = urllib.request.Request(url, method=method, data=data, headers={**UA, **(headers or {})})
        try:
            return opener.open(req, timeout=60)
        except urllib.error.HTTPError as e:
            if not follow and e.code in (301, 302, 303, 307, 308):
                return e
            if e.code == 429 or e.code >= 500:
                wait = int(e.headers.get("Retry-After") or delay)
                print(f"  {e.code} from {url}, waiting {wait} s", file=sys.stderr)
                time.sleep(wait)
                delay = min(delay * 2, 60)
                continue
            raise
        except urllib.error.URLError:
            time.sleep(delay)
            delay = min(delay * 2, 60)
    sys.exit(f"Gave up on {url}")


def get_json(url):
    with request(url) as r:
        return json.load(r)


_repo_cache = {}


def repo_info(repo):
    if repo not in _repo_cache:
        info = get_json(f"{HF}/api/models/{repo}")
        _repo_cache[repo] = info
        _repo_cache[info["id"]] = info
    return _repo_cache[repo]


def paths_info(repo, rev, paths):
    body = json.dumps({"paths": paths, "expand": False}).encode()
    with request(f"{HF}/api/models/{repo}/paths-info/{rev}", "POST", body, {"Content-Type": "application/json"}) as r:
        return {p["path"]: p for p in json.load(r)}


def head_size(repo, rev, path):
    """Size the server reports for a resolve URL, without downloading it."""
    url = f"{HF}/{repo}/resolve/{rev}/{urllib.parse.quote(path)}"
    r = request(url, "HEAD", follow=False)
    h = r.headers
    if r.status in (301, 307, 308) and not h.get("X-Linked-Size"):
        sys.exit(f"{repo}/{path}: resolve redirects ({r.status}) to {h.get('Location')}; store the canonical repo id")
    size = h.get("X-Linked-Size") or h.get("Content-Length")
    return int(size) if size else None


def sha256_small(repo, rev, path, size):
    if size > SMALL_FILE:
        sys.exit(f"{repo}/{path} isn't stored with LFS and is {size} bytes: no sha256 to pin")
    with request(f"{HF}/{repo}/resolve/{rev}/{urllib.parse.quote(path)}") as r:
        data = r.read()
    if len(data) != size:
        sys.exit(f"{repo}/{path}: downloaded {len(data)} bytes, expected {size}")
    return hashlib.sha256(data).hexdigest()


_blobs = {}


def official_sha(spec):
    """sha256 of an official file, `<org>/<repo>/<path>`. `?blobs=true` gives LFS sha256 even for
    gated repos (the tree API masks it), so no token is needed."""
    org, name, path = spec.split("/", 2)
    repo = f"{org}/{name}"
    if repo not in _blobs:
        info = get_json(f"{HF}/api/models/{repo}?blobs=true")
        _blobs[repo] = {s["rfilename"]: (s.get("lfs") or {}).get("sha256") for s in info.get("siblings", [])}
    sha = _blobs[repo].get(path)
    if not sha:
        sys.exit(f"provenance: {spec} has no published LFS sha256")
    return sha


def provenance(f, src, table):
    """Where a file's bytes come from, checked where it can be."""
    key = f"{f['repo']}/{f['path']}"
    org = f["repo"].split("/")[0]
    given = table.get(key)
    if given and given.startswith("="):
        official = given[1:]
        if official_sha(official) != f["sha256"]:
            sys.exit(f"{key}: sha256 differs from {official}")
        return f"identical to {official} (sha256 matches)"
    if given:
        return given
    if f["repo"] == src["original"] or org == src["original"].split("/")[0]:
        return "official file"
    if f["path"].endswith(".gguf"):
        return f"quantised by {org}"
    sys.exit(f"{src['id']}: {key}: say where it comes from in [provenance] (\"=<official repo>/<path>\" or plain words)")


_recipes = {}


def recipe(name):
    if name not in _recipes:
        url = f"https://raw.githubusercontent.com/leejet/stable-diffusion.cpp/{SDCPP_COMMIT}/examples/server/recipes/{name}.json"
        _recipes[name] = get_json(url)
    return _recipes[name]


def defaults_from(rec, overrides):
    d = rec.get("defaults", {})
    out = {
        "steps": d.get("steps", 20),
        "cfg": float(d.get("cfg_scale", 7.0)),
        "width": d.get("width", 1024),
        "height": d.get("height", 1024),
    }
    if "sampling_method" in d:
        out["sampler"] = d["sampling_method"]
    if "scheduler" in d:
        out["scheduler"] = d["scheduler"]
    if "distilled_guidance" in d:
        out["distilled_guidance"] = float(d["distilled_guidance"])
    if "flow_shift" in d:
        out["flow_shift"] = float(d["flow_shift"])
    if "custom_sigmas" in d:
        out["sigmas"] = [float(x) for x in d["custom_sigmas"]]
    out.update(overrides)
    if "cfg" in out:
        out["cfg"] = float(out["cfg"])
    out["server_flags"] = list(SERVER_FLAGS)
    # A recipe's load context (e.g. Qwen-Image-Edit 2511's qwen_image_zero_cond_t=true, without
    # which edits degrade badly) goes on sd-server's command line, the same as a recipe load.
    model_args = (rec.get("context") or {}).get("model_args")
    if model_args:
        out["server_flags"] += ["--model-args", model_args]
    return out


def licence_for(src, info):
    card = info.get("cardData") or {}
    hf_id = card.get("license")
    gated = info.get("gated") not in (False, None)
    if "licence" in src:
        lic = dict(src["licence"])
        if hf_id in KNOWN_LICENCES and KNOWN_LICENCES[hf_id][0] != lic["spdx"]:
            print(f"  note: {src['id']}: hand-written licence {lic['spdx']} but HF says {hf_id}", file=sys.stderr)
    elif hf_id in KNOWN_LICENCES:
        spdx, name, commercial, restricted, note = KNOWN_LICENCES[hf_id]
        lic = {"spdx": spdx, "name": name, "commercial": commercial}
        if restricted:
            lic["use_restrictions"] = True
        if note:
            lic["note"] = note
    else:
        sys.exit(f"{src['id']}: {info['id']} has licence {hf_id!r} ({card.get('license_name')}); read the model card and add `licence` to the sources file")
    lic.setdefault("url", f"{HF}/{info['id']}")
    lic["accept_required"] = gated
    return lic


def resolve_file(f, old_files):
    """Pins one file: canonical repo, revision, size, sha256."""
    info = repo_info(f["repo"])
    repo = info["id"]
    path = f["path"]
    meta = paths_info(repo, info["sha"], [path]).get(path)
    if not meta or meta.get("type") != "file":
        sys.exit(f"{repo}: {path} isn't in the repo at {info['sha']}")
    size = meta["size"]
    sha = meta["lfs"]["oid"] if meta.get("lfs") else None
    rev = info["sha"]
    old = old_files.get((repo, path))
    if old and sha and old["sha256"] == sha and old["size"] == size:
        rev = old["revision"]  # unchanged file: keep the pin
    if sha is None:
        sha = old["sha256"] if old and old["size"] == size and old["revision"] == rev else sha256_small(repo, rev, path, size)
    got = head_size(repo, rev, path)
    if got != size:
        sys.exit(f"{repo}/{path}@{rev[:10]}: HEAD says {got} bytes, the API says {size}")
    return {
        "role": f["role"],
        "category": CATEGORY[f["role"]],
        "filename": f.get("filename") or path.rsplit("/", 1)[-1],
        "repo": repo,
        "revision": rev,
        "path": path,
        "size": size,
        "sha256": sha,
    }


# ---------- TOML output (hand-rolled, to match the existing manifests) ----------

def q(v):
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, float):
        return repr(v)
    if isinstance(v, list):
        return "[" + ", ".join(q(x) for x in v) + "]"
    return json.dumps(v, ensure_ascii=False)


def table(lines, header, d, order=None):
    lines.append(header)
    keys = order or list(d)
    for k in keys:
        if k in d and d[k] is not None:
            lines.append(f"{k} = {q(d[k])}")


FILE_KEYS = ["role", "category", "filename", "repo", "revision", "path", "size", "sha256", "provenance"]
MEASURED_KEYS = ["task", "size", "gpu", "vram_total_mib", "peak_vram_mib", "cold_s", "warm_s", "need_mib", "peak_ram_mib",
                 "offload", "runtime", "integrated", "source", "note"]
TASKS = ("generate", "edit")
LICENCE_KEYS = ["spdx", "name", "url", "commercial", "use_restrictions", "accept_required", "note"]
DEFAULT_KEYS = ["steps", "cfg", "width", "height", "sampler", "scheduler", "distilled_guidance", "flow_shift", "sigmas",
                "negative_prompt", "server_flags"]


def render(m):
    lines = [
        "# Model manifest, generated by engine/scripts/gen_models.py from registry/sources/models.toml,",
        f"# the live Hugging Face API and sd-server's {m['recipe']} recipe at sd.cpp {SDCPP_COMMIT[:7]}.",
        "# Edit the sources file and re-run; only [[variant.measured]] blocks are written by hand",
        "# (re-runs keep them). Schema: engine/src/registry.rs.",
        "schema = 2",
    ]
    for k in ["id", "name", "tagline", "tags", "tasks", "backend", "original", "recipe"]:
        lines.append(f"{k} = {q(m[k])}")
    lines.append("")
    table(lines, "[licence]", m["licence"], LICENCE_KEYS)
    lines.append("")
    table(lines, "[defaults]", m["defaults"], DEFAULT_KEYS)
    for f in m["files"]:
        lines.append("")
        table(lines, "[[files]]", f, FILE_KEYS)
    for v in m["variants"]:
        lines += ["", "[[variant]]", f"tag = {q(v['tag'])}"]
        for f in v["files"]:
            lines.append("")
            table(lines, "[[variant.files]]", f, FILE_KEYS)
        for x in v["measured"]:
            lines.append("")
            table(lines, "[[variant.measured]]", x, MEASURED_KEYS)
    return "\n".join(lines) + "\n"


def old_manifest(path):
    """(files by (repo, path), measured by tag) from an existing manifest of either schema."""
    if not path.exists():
        return {}, {}, None
    old = tomllib.loads(path.read_text(encoding="utf-8"))
    files = {}
    for f in old.get("files", []):
        files[(f["repo"], f["path"])] = f
    for v in old.get("variant", []):
        for f in v.get("files", []):
            files[(f["repo"], f["path"])] = f
    if old.get("schema") == 2:
        return files, {v["tag"]: v.get("measured", []) for v in old.get("variant", [])}, old
    # Schema 1: its measurements belong to whichever variant uses its weights file.
    weights = next((f["filename"] for f in old["files"] if f["role"] in ("diffusion_model", "checkpoint")), None)
    return files, {("weights", weights): old.get("measured", [])}, old


def tasks_for(src, rec):
    """generate and/or edit, from the sources file. Edit is only allowed where sd-server's recipe
    takes reference images at the pinned commit."""
    tasks = src.get("tasks", ["generate"])
    if not tasks or any(t not in TASKS for t in tasks) or len(set(tasks)) != len(tasks):
        sys.exit(f"{src['id']}: tasks must be generate and/or edit")
    if "edit" in tasks and "ref_images" not in rec.get("features", []):
        sys.exit(f"{src['id']}: edit, but recipe {src['recipe']} has no ref_images feature at {SDCPP_COMMIT[:7]}")
    return [t for t in TASKS if t in tasks]


def build(src, root, table):
    out_path = root / "registry" / f"{src['id']}.toml"
    old_files, old_measured, old = old_manifest(out_path)
    info = repo_info(src["original"])
    rec = recipe(src["recipe"])
    files = [resolve_file(f, old_files) for f in src.get("files", [])]
    for f in files:
        f["provenance"] = provenance(f, src, table)
    variants = []
    for tag, vfiles in src["variants"].items():
        vf = [resolve_file(f, old_files) for f in vfiles]
        for f in vf:
            f["provenance"] = provenance(f, src, table)
        weights = next((f["filename"] for f in vf if f["role"] in ("diffusion_model", "checkpoint")), None)
        measured = old_measured.get(tag) or old_measured.get(("weights", weights)) or []
        variants.append({"tag": tag, "files": vf, "measured": measured})
    variants.sort(key=lambda v: sum(f["size"] for f in v["files"]))
    m = {
        "id": src["id"],
        "name": src["name"],
        "tagline": src["tagline"],
        "tags": src.get("tags", []),
        "tasks": tasks_for(src, rec),
        "backend": BACKEND,
        "original": info["id"],
        "recipe": src["recipe"],
        "licence": licence_for(src, info),
        "defaults": defaults_from(rec, src.get("defaults", {})),
        "files": sorted(files, key=lambda f: ROLE_ORDER.index(f["role"])),
        "variants": variants,
    }
    # Written, and reported, only when the text changes: a re-run on any day prints nothing new.
    text = render(m)
    m["changed"] = not out_path.exists() or out_path.read_text(encoding="utf-8") != text
    if m["changed"]:
        print(src["id"])
        report_changes(old, m)
        out_path.write_text(text, encoding="utf-8", newline="\n")
    return m


def report_changes(old, m):
    if old is None:
        print("  new manifest")
        return
    if old.get("schema") != 2:
        print("  schema 1 -> 2")
        return
    print("  (file, licence, defaults and tag changes below; anything else is notes, provenance or comments)")
    olds = {f["path"]: f for f in old.get("files", []) + [f for v in old.get("variant", []) for f in v.get("files", [])]}
    news = {f["path"]: f for f in m["files"] + [f for v in m["variants"] for f in v["files"]]}
    for p in sorted(set(olds) | set(news)):
        a, b = olds.get(p), news.get(p)
        if a is None:
            print(f"  + {p}")
        elif b is None:
            print(f"  - {p}")
        elif (a["sha256"], a["revision"], a["size"]) != (b["sha256"], b["revision"], b["size"]):
            print(f"  ~ {p}: revision {a['revision'][:10]} -> {b['revision'][:10]}, sha256 {a['sha256'][:10]} -> {b['sha256'][:10]}")
    for k in ["licence", "defaults", "tasks"]:
        if old.get(k) != m[k]:
            print(f"  ~ {k}: {old.get(k)} -> {m[k]}")
    if [v["tag"] for v in old.get("variant", [])] != [v["tag"] for v in m["variants"]]:
        print(f"  ~ tags: {[v['tag'] for v in old.get('variant', [])]} -> {[v['tag'] for v in m['variants']]}")


def check_shared(models):
    """The engine refuses two manifests that put different bytes at the same local path."""
    seen = {}
    for m in models:
        for f in m["files"] + [f for v in m["variants"] for f in v["files"]]:
            key = f"{f['category']}/{f['filename']}"
            if key in seen and (seen[key][1]["sha256"], seen[key][1]["size"]) != (f["sha256"], f["size"]):
                sys.exit(f"{m['id']} and {seen[key][0]} both write {key} with different contents: set `filename` in the sources")
            seen.setdefault(key, (m["id"], f))


def main():
    root = pathlib.Path(__file__).resolve().parents[2]
    doc = tomllib.loads((root / "registry" / "sources" / "models.toml").read_text(encoding="utf-8"))
    sources, table = doc["model"], doc.get("provenance", {})
    ids = [s["id"] for s in sources]
    if len(ids) != len(set(ids)):
        sys.exit("duplicate ids in the sources file")
    only = set(sys.argv[1:])
    unknown = only - set(ids)
    if unknown:
        sys.exit(f"not in the sources file: {', '.join(sorted(unknown))}")
    built = [build(s, root, table) for s in sources if not only or s["id"] in only]
    if not only:
        check_shared(built)
        stale = {p.stem for p in (root / "registry").glob("*.toml")} - set(ids)
        for s in sorted(stale):
            print(f"note: registry/{s}.toml isn't in the sources file (delete it if it's gone)")
    changed = sum(m["changed"] for m in built)
    print(f"{len(built)} manifests checked: {changed} changed" if changed else f"{len(built)} manifests checked: no changes")


if __name__ == "__main__":
    main()
