from pathlib import Path
from urllib.request import Request, urlopen
import html
import xml.etree.ElementTree as ET

OUT_DIR = Path("assets/social")

ICONS = {
    "discord": {
        "url": "https://api.iconify.design/logos/discord-icon.svg",
        "size": 31,
    },
    "tiktok": {
        "url": "https://api.iconify.design/logos/tiktok-icon.svg",
        "size": 31,
    },
    "facebook": {
        "url": "https://api.iconify.design/logos/facebook.svg",
        "size": 30,
    },
    "steam": {
        "url": "https://api.iconify.design/simple-icons/steam.svg?color=%2366C0F4",
        "size": 31,
    },
    "spotify": {
        "url": "https://api.iconify.design/simple-icons/spotify.svg?color=%231ED760",
        "size": 32,
    },
    "paypal": {
        "url": "https://api.iconify.design/logos/paypal.svg",
        "size": 29,
    },
}


def fetch(url: str) -> str:
    req = Request(
        url,
        headers={
            "User-Agent": "Mozilla/5.0",
            "Accept": "image/svg+xml,*/*",
        },
    )

    with urlopen(req, timeout=20) as res:
        return res.read().decode("utf-8")


def strip_namespaces(root):
    for element in root.iter():
        if "}" in element.tag:
            element.tag = element.tag.split("}", 1)[1]


def wrap_svg(source: str, icon_size: int) -> str:
    root = ET.fromstring(source)
    strip_namespaces(root)

    view_box = root.attrib.get("viewBox")

    if not view_box:
        width = root.attrib.get("width", "24").replace("px", "")
        height = root.attrib.get("height", "24").replace("px", "")
        view_box = f"0 0 {width} {height}"

    inherited = []

    for key, value in root.attrib.items():
        if key not in {"viewBox", "width", "height"}:
            inherited.append(
                f'{html.escape(key)}="{html.escape(value, quote=True)}"'
            )

    inherited_attrs = " ".join(inherited)

    inner = "".join(
        ET.tostring(child, encoding="unicode")
        for child in root
    )

    x = (56 - icon_size) / 2
    y = (56 - icon_size) / 2

    return f"""<svg
  xmlns="http://www.w3.org/2000/svg"
  width="56"
  height="56"
  viewBox="0 0 56 56"
>
  <defs>
    <linearGradient id="border" x1="5" y1="3" x2="51" y2="53">
      <stop offset="0" stop-color="#3D444D"/>
      <stop offset="0.48" stop-color="#30363D"/>
      <stop offset="1" stop-color="#21262D"/>
    </linearGradient>

    <linearGradient id="surface" x1="6" y1="4" x2="48" y2="52">
      <stop offset="0" stop-color="#1B222C"/>
      <stop offset="1" stop-color="#151A21"/>
    </linearGradient>
  </defs>

  <!-- unified tile -->
  <rect
    x="0.75"
    y="0.75"
    width="54.5"
    height="54.5"
    rx="13"
    fill="url(#surface)"
    stroke="url(#border)"
    stroke-width="1.5"
  />

  <!-- subtle top highlight -->
  <path
    d="M14 1.5H42C49.5 1.5 54.5 6.5 54.5 14"
    fill="none"
    stroke="#FFFFFF"
    stroke-opacity="0.035"
    stroke-width="1"
  />

  <!-- actual brand icon -->
  <svg
    x="{x}"
    y="{y}"
    width="{icon_size}"
    height="{icon_size}"
    viewBox="{html.escape(view_box, quote=True)}"
    preserveAspectRatio="xMidYMid meet"
    {inherited_attrs}
  >
    {inner}
  </svg>
</svg>
"""


def main():
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    for name, config in ICONS.items():
        print(f"Fetching {name}...")

        source = fetch(config["url"])
        result = wrap_svg(source, config["size"])

        output = OUT_DIR / f"{name}.svg"
        output.write_text(result, encoding="utf-8")

        print(f"  -> {output}")

    print("\nDone.")


if __name__ == "__main__":
    main()
