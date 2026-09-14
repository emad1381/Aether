package com.aether.mobile

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Intent
import android.net.VpnService
import android.os.Build

/**
 * The tun device Android hands out. Everything on the phone is routed into it
 * except this app, whose sockets must stay outside so the core can reach the
 * Cloudflare edge and its own SOCKS listener.
 */
class AetherVpnService : VpnService() {
    private var tunReady = false

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        showNotification()
        if (tunReady) return START_STICKY

        val builder = Builder()
            .setSession("Aether")
            .setMtu(MTU)
            .addAddress(TUN_V4, 32)
            .addAddress(TUN_V6, 128)
            .addDnsServer(UPSTREAM_DNS)
            .addRoute("0.0.0.0", 0)
            .addRoute("::", 0)
            .setBlocking(false)

        runCatching { builder.addDisallowedApplication(packageName) }

        val descriptor = runCatching { builder.establish() }.getOrNull()
        if (descriptor == null) {
            AetherVpn.report(2)
            stopSelf()
            return START_NOT_STICKY
        }

        // Ownership of the descriptor moves to the Rust relay, which closes it
        // when the session ends.
        val fd = descriptor.detachFd()
        tunReady = AetherVpn.onTunReady(fd)
        if (!tunReady) {
            AetherVpn.report(2)
            stopSelf()
            return START_NOT_STICKY
        }
        return START_STICKY
    }

    override fun onDestroy() {
        tunReady = false
        AetherVpn.report(3)
        super.onDestroy()
    }

    override fun onRevoke() {
        tunReady = false
        AetherVpn.report(3)
        stopSelf()
        super.onRevoke()
    }

    private fun showNotification() {
        val manager = getSystemService(NotificationManager::class.java)
        val notification = runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                if (manager?.getNotificationChannel(CHANNEL) == null) {
                    manager?.createNotificationChannel(
                        NotificationChannel(CHANNEL, "Aether VPN", NotificationManager.IMPORTANCE_LOW)
                    )
                }
                Notification.Builder(this, CHANNEL)
            } else {
                @Suppress("DEPRECATION")
                Notification.Builder(this)
            }
                .setContentTitle("Aether")
                .setContentText("System-wide tunnel is active")
                .setSmallIcon(android.R.drawable.ic_lock_lock)
                .setOngoing(true)
                .build()
        }.getOrNull() ?: return
        runCatching { startForeground(NOTIF_ID, notification) }
    }

    private companion object {
        const val CHANNEL = "aether-vpn"
        const val NOTIF_ID = 0xA37
        const val MTU = 1500
        const val TUN_V4 = "10.111.0.2"
        const val TUN_V6 = "fd00:111::2"
        const val UPSTREAM_DNS = "1.1.1.1"
    }
}
