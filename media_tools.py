"""Outils médias pour Hermes Map : vidéos illisibles par le navigateur (conversion ffmpeg mise en cache),
documents Office (Word -> HTML via textutil, l'outil intégré à macOS), ouverture dans l'app du Mac."""
import hashlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

# formats que les navigateurs lisent directement (H.264/H.265/VP9/AV1 dans ces conteneurs)
WEB_VIDEO_EXT = {".mp4", ".m4v", ".webm", ".mov", ".ogv"}
# formats vidéo qu'il faut convertir
OTHER_VIDEO_EXT = {".avi", ".mkv", ".wmv", ".flv", ".mpg", ".mpeg", ".m2v", ".3gp", ".3g2", ".ts", ".mts",
                   ".m2ts", ".vob", ".divx", ".xvid", ".rm", ".rmvb", ".asf", ".f4v"}
OFFICE_EXT = {".doc", ".docx", ".rtf", ".rtfd", ".odt", ".pages", ".xls", ".xlsx", ".ods", ".numbers",
              ".ppt", ".pptx", ".odp", ".key", ".csv", ".epub"}
TEXTUTIL_EXT = {".doc", ".docx", ".rtf", ".rtfd", ".odt", ".webarchive", ".wordml"}

CACHE = (Path(os.path.expanduser("~/Library/Caches/hermes-map")) if sys.platform == "darwin"
         else Path(tempfile.gettempdir()) / "hermes-map")
VIDEO_CACHE = CACHE / "videos"
DOC_CACHE = CACHE / "docs"


def find_ffmpeg():
    for c in (shutil.which("ffmpeg"), "/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg",
              os.path.expanduser("~/bin/ffmpeg")):
        if c and Path(c).exists():
            return c
    return None


def _key(p: Path):
    st = p.stat()
    return hashlib.sha1(f"{p}|{st.st_size}|{st.st_mtime}".encode()).hexdigest()


class VideoConverter:
    """Convertit (une seule à la fois) les vidéos illisibles par le navigateur en MP4, gardé en cache."""

    def __init__(self):
        self.ffmpeg = find_ffmpeg()
        self.jobs = {}          # clé -> {state, progress, error}
        self.lock = threading.Lock()
        self.sem = threading.Semaphore(1)

    def status(self, p: Path):
        ext = p.suffix.lower()
        if ext in WEB_VIDEO_EXT:
            return {"state": "native"}
        key = _key(p)
        out = VIDEO_CACHE / (key + ".mp4")
        if out.exists() and out.stat().st_size > 0:
            return {"state": "ready", "file": str(out)}
        if not self.ffmpeg:
            return {"state": "no-ffmpeg"}
        with self.lock:
            job = self.jobs.get(key)
            if job is None:
                job = {"state": "converting", "progress": 0.0, "started": time.time()}
                self.jobs[key] = job
                threading.Thread(target=self._convert, args=(p, out, job), daemon=True).start()
        return {k: v for k, v in job.items() if k != "started"}

    def cached_file(self, p: Path):
        out = VIDEO_CACHE / (_key(p) + ".mp4")
        return out if out.exists() and out.stat().st_size > 0 else None

    def _duration(self, p: Path):
        try:
            r = subprocess.run([self.ffmpeg, "-hide_banner", "-i", str(p)], capture_output=True, text=True, timeout=30)
            m = re.search(r"Duration:\s*(\d+):(\d+):(\d+\.?\d*)", r.stderr)
            if m:
                return int(m.group(1)) * 3600 + int(m.group(2)) * 60 + float(m.group(3))
        except (OSError, subprocess.SubprocessError):
            pass
        return None

    def _convert(self, src: Path, out: Path, job):
        with self.sem:
            VIDEO_CACHE.mkdir(parents=True, exist_ok=True)
            tmp = out.with_suffix(".part.mp4")
            dur = self._duration(src)
            # 1) réempaquetage rapide (sans réencoder la vidéo) si le flux est déjà en H.264/H.265
            # 2) sinon réencodage H.264 + AAC
            attempts = [
                ["-c:v", "copy", "-c:a", "aac", "-b:a", "160k"],
                ["-c:v", "libx264", "-preset", "veryfast", "-crf", "23", "-pix_fmt", "yuv420p",
                 "-c:a", "aac", "-b:a", "160k"],
            ]
            for args in attempts:
                cmd = [self.ffmpeg, "-hide_banner", "-y", "-i", str(src), "-map", "0:v:0", "-map", "0:a:0?",
                       *args, "-movflags", "+faststart", "-progress", "pipe:1", "-nostats", str(tmp)]
                try:
                    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
                    for line in proc.stdout:
                        if line.startswith("out_time_ms=") and dur:
                            try:
                                job["progress"] = min(0.99, int(line.split("=")[1]) / 1e6 / dur)
                            except ValueError:
                                pass
                    code = proc.wait()
                except OSError as e:
                    job.update(state="error", error=str(e))
                    return
                if code == 0 and tmp.exists() and tmp.stat().st_size > 0:
                    if args[1] == "copy" and not self._probe_ok(tmp):
                        continue      # codec incompatible avec le navigateur : on réencode
                    tmp.replace(out)
                    job.update(state="ready", progress=1.0, file=str(out))
                    return
            job.update(state="error", error="ffmpeg n'a pas pu convertir cette vidéo")
            try:
                tmp.unlink()
            except OSError:
                pass

    def _probe_ok(self, mp4: Path):
        """Le réempaquetage n'est utile que si la vidéo est en H.264 / H.265 (lisibles par les navigateurs)."""
        try:
            r = subprocess.run([self.ffmpeg, "-hide_banner", "-i", str(mp4)], capture_output=True, text=True, timeout=30)
            return bool(re.search(r"Video:\s*(h264|hevc)", r.stderr))
        except (OSError, subprocess.SubprocessError):
            return False


def doc_html(p: Path):
    """Word / RTF / ODT -> HTML avec textutil (intégré à macOS). Renvoie le HTML ou None."""
    ext = p.suffix.lower()
    if ext not in TEXTUTIL_EXT or not shutil.which("textutil"):
        return None
    DOC_CACHE.mkdir(parents=True, exist_ok=True)
    out = DOC_CACHE / (_key(p) + ".html")
    if not out.exists():
        try:
            subprocess.run(["textutil", "-convert", "html", "-output", str(out), str(p)],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60)
        except (OSError, subprocess.SubprocessError):
            return None
    try:
        return out.read_text("utf-8", errors="replace")
    except OSError:
        return None


def open_in_app(p: Path, reveal=False):
    """Ouvre le fichier avec l'app par défaut du Mac (ou le montre dans le Finder)."""
    if sys.platform != "darwin":
        raise RuntimeError("disponible seulement sur Mac")
    subprocess.Popen(["open", "-R", str(p)] if reveal else ["open", str(p)])


def choose_folder(prompt="Choisis un dossier"):
    """Fenêtre « Choisir un dossier » du Mac. Renvoie {"path": …} ou {"cancel": True}."""
    if sys.platform != "darwin" or not shutil.which("osascript"):
        return {"error": "sélecteur de dossier disponible seulement sur Mac : colle le chemin à la main"}
    safe = prompt.replace("\\", "").replace('"', "'")
    script = f'activate\nPOSIX path of (choose folder with prompt "{safe}")'
    try:
        r = subprocess.run(["osascript", "-e", script], capture_output=True, text=True, timeout=600)
    except (OSError, subprocess.SubprocessError) as e:
        return {"error": str(e)}
    if r.returncode != 0:
        return {"cancel": True}
    return {"path": r.stdout.strip().rstrip("/") or "/"}


def open_privacy_settings(app_path=None):
    """Ouvre Réglages Système > Accès complet au disque (et montre l'app dans le Finder)."""
    if sys.platform != "darwin":
        raise RuntimeError("disponible seulement sur Mac")
    if app_path and Path(app_path).exists():
        subprocess.Popen(["open", "-R", app_path])
        time.sleep(0.4)
    subprocess.Popen(["open", "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"])
