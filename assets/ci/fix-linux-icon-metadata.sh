#!/usr/bin/env bash
# Inject the canonical Freminal desktop entry (carrying StartupWMClass, which
# cargo-bundle's generated template omits) into the bundled .deb and .AppImage
# artifacts.
#
# Background
# ----------
# cargo-bundle's desktop-file generator (src/bundle/linux/common.rs) is
# hardcoded and emits NO `StartupWMClass` line, and exposes no template hook.
# Without `StartupWMClass=freminal` matching the runtime app_id/WM_CLASS set in
# freminal/src/gui/run.rs, GNOME / KDE / wlroots taskbars cannot associate the
# live window with its desktop entry, so they fall back to a generic terminal
# icon.  This script rewrites the generated desktop file in each artifact with
# the repository's canonical assets/freminal.desktop.
#
# The non-square icon-size problem (a separate root cause) is fixed at the
# asset level by listing assets/icon.svg in the cargo-bundle `icon` list, which
# lands the icon in hicolor/scalable/apps -- so this script only touches the
# desktop file.
#
# Requirements: dpkg-deb, unsquashfs + mksquashfs (squashfs-tools), python3.
#
# Usage:
#   fix-linux-icon-metadata.sh <path-to.deb> <path-to.AppImage>
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_SRC="${SCRIPT_DIR}/../freminal.desktop"

if [[ ! -f "${DESKTOP_SRC}" ]]; then
	echo "error: canonical desktop file not found at ${DESKTOP_SRC}" >&2
	exit 1
fi

DEB_PATH="${1:?usage: fix-linux-icon-metadata.sh <deb> <appimage>}"
APPIMAGE_PATH="${2:?usage: fix-linux-icon-metadata.sh <deb> <appimage>}"

# ── Patch the .deb ───────────────────────────────────────────────────────────
# dpkg-deb -R / -b round-trips the package and regenerates md5sums, so simply
# overwriting the desktop file in the extracted tree is sufficient.
# --root-owner-group keeps the rebuilt package's files owned by root:root.
patch_deb() {
	local deb="$1"
	echo "::group::Patching desktop entry in $(basename "${deb}")"
	local workdir
	workdir="$(mktemp -d)"
	dpkg-deb -R "${deb}" "${workdir}"

	local dest="${workdir}/usr/share/applications/freminal.desktop"
	mkdir -p "$(dirname "${dest}")"
	cp "${DESKTOP_SRC}" "${dest}"
	echo "Installed desktop entry:"
	cat "${dest}"

	dpkg-deb --root-owner-group -b "${workdir}" "${deb}"
	rm -rf "${workdir}"
	echo "::endgroup::"
}

# ── Patch the .AppImage ──────────────────────────────────────────────────────
# cargo-bundle's AppImage is a static type2 runtime header followed by a
# squashfs payload.  That runtime does NOT implement --appimage-extract (the
# argument is passed straight through to AppRun, i.e. the freminal binary), so
# the only reliable way in is to operate on the squashfs directly:
#
#   1. Compute the squashfs offset = end of the leading ELF runtime (the
#      section-header table is last in this runtime, so the payload starts at
#      e_shoff + e_shnum * e_shentsize -- verified to land on the `hsqs` magic).
#   2. Split off the runtime header bytes verbatim.
#   3. unsquashfs the payload at that offset.
#   4. Replace the desktop entry (both the AppDir-root copy that integrators
#      read and the usr/share/applications copy).
#   5. Re-mksquashfs and re-prepend the original runtime header.
patch_appimage() {
	local appimage="$1"
	echo "::group::Patching desktop entry in $(basename "${appimage}")"
	local workdir
	workdir="$(mktemp -d)"
	local appimage_abs
	appimage_abs="$(readlink -f "${appimage}")"

	local offset
	offset="$(
		python3 - "${appimage_abs}" <<'PY'
import struct, sys

with open(sys.argv[1], "rb") as f:
    head = f.read(64)
if head[:4] != b"\x7fELF":
    sys.exit("error: AppImage does not start with an ELF runtime header")
e_shoff = struct.unpack_from("<Q", head, 0x28)[0]
e_shentsize = struct.unpack_from("<H", head, 0x3A)[0]
e_shnum = struct.unpack_from("<H", head, 0x3C)[0]
print(e_shoff + e_shentsize * e_shnum)
PY
	)"

	# Sanity-check: the computed offset must land on the squashfs magic.
	local magic
	magic="$(dd if="${appimage_abs}" bs=1 skip="${offset}" count=4 status=none)"
	if [[ "${magic}" != "hsqs" ]]; then
		echo "error: no squashfs magic at offset ${offset} (got '${magic}')" >&2
		exit 1
	fi
	echo "squashfs offset: ${offset}"

	dd if="${appimage_abs}" of="${workdir}/runtime" bs="${offset}" count=1 status=none

	local appdir="${workdir}/squashfs-root"
	unsquashfs -o "${offset}" -d "${appdir}" "${appimage_abs}" >/dev/null

	cp "${DESKTOP_SRC}" "${appdir}/freminal.desktop"
	if [[ -e "${appdir}/usr/share/applications/freminal.desktop" ]]; then
		cp "${DESKTOP_SRC}" "${appdir}/usr/share/applications/freminal.desktop"
	fi
	echo "Installed desktop entry:"
	cat "${appdir}/freminal.desktop"

	mksquashfs "${appdir}" "${workdir}/payload.squashfs" \
		-root-owned -noappend -quiet
	cat "${workdir}/runtime" "${workdir}/payload.squashfs" >"${appimage}"
	chmod +x "${appimage}"

	rm -rf "${workdir}"
	echo "::endgroup::"
}

patch_deb "${DEB_PATH}"
patch_appimage "${APPIMAGE_PATH}"
