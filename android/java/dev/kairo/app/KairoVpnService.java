package dev.kairo.app;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.net.VpnService;
import android.os.ParcelFileDescriptor;

import java.util.concurrent.CompletableFuture;

/**
 * The VPN: it makes the tun device and keeps it (and a notification) alive. The
 * Rust core, in this same process, is given the descriptor and does the routing.
 *
 * Only the engine builds the tun ({@link Bridge#establish()}): every other way the
 * service starts (a restart after Android killed the process, Always-on VPN, boot,
 * an update, the Quick Settings tile) goes into the foreground and asks the engine
 * to connect, which comes back here for the tun. A tun with no core behind it
 * would take the phone's network with it.
 */
public class KairoVpnService extends VpnService {
    /** From {@link Bridge#establish()} when the service is not running yet: build the tun. */
    static final String START = "dev.kairo.app.START";
    static final String STOP = "dev.kairo.app.STOP";
    /** Turn Kairo on (the tile). */
    static final String CONNECT = "dev.kairo.app.CONNECT";
    /** Back on if it was on (boot, an update). */
    static final String RESTORE = "dev.kairo.app.RESTORE";
    private static final String CHANNEL = "kairo_vpn";
    private static final int NOTIFICATION = 1;

    /** The answer {@link Bridge#establish()} waits for: the descriptor, or a negative code. */
    static volatile CompletableFuture<Integer> pending;
    /** The service while it is created (it may have no tun yet). */
    private static volatile KairoVpnService alive;

    private ParcelFileDescriptor tun;

    /** Starts the service in the foreground with `action` (CONNECT or RESTORE). */
    static void begin(Context c, String action) {
        c.startForegroundService(new Intent(c, KairoVpnService.class).setAction(action));
    }

    static void stop(Context c) {
        KairoVpnService s = alive;
        if (s != null) {
            s.close();
        }
    }

    /**
     * The tun from the service already running (in the foreground since it started),
     * without starting it again from the background: the descriptor, or -2.
     */
    static int establishInRunning() {
        KairoVpnService s = alive;
        return s == null ? -3 : s.establishTun();
    }

    @Override
    public void onCreate() {
        super.onCreate();
        alive = this;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent == null ? null : intent.getAction();
        if (STOP.equals(action)) {
            close();
            return START_NOT_STICKY;
        }
        startInForeground();
        if (START.equals(action)) {
            CompletableFuture<Integer> answer = pending;
            int fd = establishTun();
            if (answer != null) {
                answer.complete(fd);
            }
            if (fd < 0) {
                close();
            }
        } else if (CONNECT.equals(action) || SERVICE_INTERFACE.equals(action)) {
            // The tile, or Always-on VPN (the system wants it on).
            Bridge.nativeConnect(true);
        } else {
            // Restarted after Android killed the process (null), boot or an update:
            // on again if it was on, else gone.
            if (!Bridge.nativeRestore()) {
                close();
                return START_NOT_STICKY;
            }
        }
        // Android starts it again after killing the process; turning Kairo off
        // stops it, which ends that.
        return START_STICKY;
    }

    private int establishTun() {
        try {
            Builder b = new Builder()
                    .setSession("Kairo")
                    .setMtu(1500)
                    .addAddress("172.19.0.1", 30)
                    .addRoute("0.0.0.0", 0)
                    .addAddress("fdfe:dcba:9876::1", 126)
                    .addRoute("::", 0)
                    .addDnsServer("172.19.0.2")
                    .setMetered(false);
            // The core and Kairo's own requests must not loop back into the VPN.
            b.addDisallowedApplication(getPackageName());
            ParcelFileDescriptor made = b.establish();
            if (made == null) {
                return -2;
            }
            ParcelFileDescriptor old = tun;
            tun = made;
            if (old != null) {
                try { old.close(); } catch (Exception ignored) { }
            }
            return made.getFd();
        } catch (Exception e) {
            android.util.Log.w("Kairo", "establish failed", e);
            return -2;
        }
    }

    private void startInForeground() {
        NotificationManager nm = getSystemService(NotificationManager.class);
        nm.createNotificationChannel(new NotificationChannel(CHANNEL, "Kairo is on", NotificationManager.IMPORTANCE_LOW));
        Intent open = getPackageManager().getLaunchIntentForPackage(getPackageName());
        PendingIntent tap = PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_IMMUTABLE);
        PendingIntent off = PendingIntent.getBroadcast(this, 0,
                new Intent(this, DisconnectReceiver.class).setPackage(getPackageName()), PendingIntent.FLAG_IMMUTABLE);
        Notification n = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(Bridge.mark(this))
                .setContentTitle("Kairo is on")
                .setContentIntent(tap)
                .addAction(new Notification.Action.Builder(null, "Disconnect", off).build())
                .setOngoing(true)
                .build();
        startForeground(NOTIFICATION, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
    }

    private void close() {
        ParcelFileDescriptor t = tun;
        tun = null;
        if (t != null) {
            try { t.close(); } catch (Exception ignored) { }
        }
        stopForeground(STOP_FOREGROUND_REMOVE);
        stopSelf();
    }

    @Override
    public void onRevoke() {
        close();
        Bridge.nativeRevoked();
    }

    @Override
    public void onDestroy() {
        close();
        if (alive == this) {
            alive = null;
        }
        super.onDestroy();
    }
}
