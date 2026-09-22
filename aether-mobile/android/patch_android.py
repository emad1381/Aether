#!/usr/bin/env python3
"""Injects the Aether VpnService into the Gradle project `tauri android init`
generates.

The mobile app owns a real system-wide VPN, so the Android project needs the
Kotlin bridge, the service declaration and the runtime permissions. The project
itself is generated in CI, so it is patched here rather than committed.

Usage: python3 aether-mobile/android/patch_android.py <gen/android>
"""

import os
import shutil
import sys

PACKAGE = "com.aether.mobile"
SOURCES = ["AetherVpn.kt", "AetherVpnService.kt", "AetherBoot.kt"]

PERMISSIONS = [
    "android.permission.INTERNET",
    "android.permission.ACCESS_NETWORK_STATE",
    "android.permission.FOREGROUND_SERVICE",
    "android.permission.FOREGROUND_SERVICE_SPECIAL_USE",
    "android.permission.FOREGROUND_SERVICE_VPN",
    "android.permission.POST_NOTIFICATIONS",
]

COMPONENTS = """
        <!-- Aether system-wide VPN: the tun descriptor is handed to the in-process core. -->
        <service
            android:name="{package}.AetherVpnService"
            android:exported="false"
            android:foregroundServiceType="specialUse|vpn"
            android:permission="android.permission.BIND_VPN_SERVICE">
            <intent-filter>
                <action android:name="android.net.VpnService" />
            </intent-filter>
            <property
                android:name="android.app.PROPERTY_SPECIAL_USE_FGS_SUBTYPE"
                android:value="vpn" />
        </service>

        <!-- Boots the VPN bridge before any activity exists. -->
        <provider
            android:name="{package}.AetherBoot"
            android:authorities="{package}.boot"
            android:exported="false" />
""".format(package=PACKAGE)


def die(message):
    print("patch_android: " + message, file=sys.stderr)
    sys.exit(1)


def copy_sources(here, java_dir):
    os.makedirs(java_dir, exist_ok=True)
    for name in SOURCES:
        source = os.path.join(here, name)
        if not os.path.isfile(source):
            die("missing source %s" % source)
        shutil.copyfile(source, os.path.join(java_dir, name))
        print("patch_android: copied %s" % name)


def patch_manifest(path):
    if not os.path.isfile(path):
        die("no manifest at %s" % path)
    with open(path, "r", encoding="utf-8") as handle:
        text = handle.read()
    if "AetherVpnService" in text:
        print("patch_android: manifest already patched")
        return

    closing_application = text.rfind("</application>")
    if closing_application < 0:
        die("the manifest has no </application>")
    text = text[:closing_application] + COMPONENTS + text[closing_application:]

    added = []
    for permission in PERMISSIONS:
        if permission in text:
            continue
        added.append('    <uses-permission android:name="%s" />' % permission)
    if added:
        closing_manifest = text.rfind("</manifest>")
        if closing_manifest < 0:
            die("the manifest has no </manifest>")
        block = "\n".join(added) + "\n"
        text = text[:closing_manifest] + block + text[closing_manifest:]

    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    print("patch_android: service, provider and %d permission(s) added" % len(added))


def main(argv):
    if len(argv) != 2:
        die("usage: patch_android.py <gen/android>")
    project = argv[1]
    if not os.path.isdir(project):
        die("%s is not a directory; run `cargo tauri android init` first" % project)
    here = os.path.dirname(os.path.abspath(__file__))
    java_dir = os.path.join(
        project, "app", "src", "main", "java", *PACKAGE.split(".")
    )
    manifest = os.path.join(project, "app", "src", "main", "AndroidManifest.xml")
    copy_sources(here, java_dir)
    patch_manifest(manifest)
    print("patch_android: done")


if __name__ == "__main__":
    main(sys.argv)
