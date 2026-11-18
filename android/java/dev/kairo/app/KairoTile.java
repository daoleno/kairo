package dev.kairo.app;

import android.app.KeyguardManager;
import android.app.PendingIntent;
import android.content.Intent;
import android.graphics.drawable.Icon;
import android.net.VpnService;
import android.os.Build;
import android.service.quicksettings.Tile;
import android.service.quicksettings.TileService;

/**
 * Kairo in Quick Settings: one tap turns it on or off, without opening the app. It
 * shows the engine's state ({@link Bridge#state}), whichever way Kairo was switched.
 * What only the app can do (a first subscription, Android's VPN permission) opens
 * Kairo at that step; after that the tile works alone. A long press opens Kairo
 * (the launcher activity answers QS_TILE_PREFERENCES).
 */
public class KairoTile extends TileService {
    /** The tile while the shade shows it. */
    private static volatile KairoTile listening;

    /** Shows the engine's state on the tile, if the shade shows it (main thread). */
    static void refreshShown() {
        KairoTile t = listening;
        if (t != null) {
            t.refresh();
        }
    }

    @Override
    public void onStartListening() {
        super.onStartListening();
        listening = this;
        refresh();
    }

    @Override
    public void onStopListening() {
        if (listening == this) {
            listening = null;
        }
        super.onStopListening();
    }

    @Override
    public void onTileAdded() {
        Bridge.tileAdded(true);
    }

    @Override
    public void onTileRemoved() {
        Bridge.tileAdded(false);
    }

    @Override
    public void onClick() {
        KeyguardManager kg = getSystemService(KeyguardManager.class);
        if (kg != null && kg.isKeyguardLocked()) {
            unlockAndRun(this::toggle);
        } else {
            toggle();
        }
    }

    private void toggle() {
        int s = Bridge.state;
        if (s == Bridge.ON || s == Bridge.CONNECTING) {
            Bridge.nativeConnect(false);
            show(Bridge.OFF);
        } else if (s == Bridge.UNAVAILABLE) {
            openKairo();
        } else if (VpnService.prepare(this) != null) {
            // Android asks for the VPN permission over Kairo, once.
            Bridge.connectWhenOpen = true;
            openKairo();
        } else {
            KairoVpnService.begin(this, KairoVpnService.CONNECT);
            show(Bridge.CONNECTING);
        }
    }

    private void openKairo() {
        Intent open = getPackageManager().getLaunchIntentForPackage(getPackageName());
        if (open == null) {
            return;
        }
        open.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        if (Build.VERSION.SDK_INT >= 34) {
            startActivityAndCollapse(PendingIntent.getActivity(this, 3, open,
                    PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT));
        } else {
            startActivityAndCollapse(open);
        }
    }

    private void refresh() {
        show(Bridge.state);
    }

    private void show(int s) {
        Tile tile = getQsTile();
        if (tile == null) {
            return;
        }
        tile.setLabel(Bridge.NAME);
        tile.setIcon(Icon.createWithResource(this, Bridge.mark(this)));
        tile.setState(s == Bridge.ON || s == Bridge.CONNECTING ? Tile.STATE_ACTIVE : Tile.STATE_INACTIVE);
        if (Build.VERSION.SDK_INT >= 29) {
            tile.setSubtitle(Bridge.subtitleOf(s));
        }
        tile.setContentDescription(Bridge.NAME);
        tile.updateTile();
    }
}
