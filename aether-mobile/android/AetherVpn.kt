package com.aether.mobile

import android.content.Context
import android.content.Intent
import android.net.VpnService
import android.os.Handler
import android.os.Looper

/**
 * Drives the system-wide VPN.
 *
 * The Rust side never calls into Java: it only flips two flags, which this
 * object polls. Everything Android specific - the consent dialog, the
 * VpnService lifetime, the tun descriptor - happens here.
 */
object AetherVpn {
    private const val POLL_MS = 300L
    private const val CONSENT_TIMEOUT_MS = 120_000L

    private val handler = Handler(Looper.getMainLooper())

    @Volatile private var appContext: Context? = null
    private var waitingForConsent = false
    private var consentDeadline = 0L

    private external fun nativePoll(): Int
    private external fun nativeOnTunReady(fd: Int): Int
    private external fun nativeReport(code: Int)

    init {
        runCatching { System.loadLibrary("aether_mobile_lib") }
            .recoverCatching { System.loadLibrary("aether_mobile") }
    }

    /** Called from the boot provider, before any activity exists. */
    fun bootstrap(context: Context) {
        appContext = context.applicationContext
        handler.removeCallbacks(tick)
        handler.postDelayed(tick, POLL_MS)
    }

    /** True when the relay accepted the descriptor. */
    fun onTunReady(fd: Int): Boolean = nativeOnTunReady(fd) == 0

    fun report(code: Int) {
        runCatching { nativeReport(code) }
    }

    private val tick = object : Runnable {
        override fun run() {
            runCatching {
                if (waitingForConsent) checkConsent()
                val work = nativePoll()
                if (work and 1 != 0) begin()
                if (work and 2 != 0) stopService()
            }
            handler.postDelayed(this, POLL_MS)
        }
    }

    private fun begin() {
        val context = appContext ?: return report(4)
        val consent = runCatching { VpnService.prepare(context) }.getOrNull()
        if (consent == null) {
            report(0)
            startService(context)
            return
        }
        // First run: the system shows the "allow VPN" dialog, and prepare()
        // reports null once the user accepts.
        val launched = runCatching {
            context.startActivity(consent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        }.isSuccess
        if (!launched) return report(4)
        waitingForConsent = true
        consentDeadline = System.currentTimeMillis() + CONSENT_TIMEOUT_MS
    }

    private fun checkConsent() {
        val context = appContext ?: return
        val granted = runCatching { VpnService.prepare(context) == null }.getOrDefault(false)
        if (granted) {
            waitingForConsent = false
            report(0)
            startService(context)
            return
        }
        if (System.currentTimeMillis() > consentDeadline) {
            waitingForConsent = false
            report(1)
        }
    }

    private fun startService(context: Context) {
        val intent = Intent(context, AetherVpnService::class.java)
        runCatching { context.startService(intent) }
            .onFailure { report(2) }
    }

    private fun stopService() {
        val context = appContext ?: return
        runCatching { context.stopService(Intent(context, AetherVpnService::class.java)) }
    }
}
