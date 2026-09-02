package com.flint.game;

import android.view.InputDevice;
import android.view.MotionEvent;
import com.google.androidgamesdk.GameActivity;

/**
 * GameActivity subclass that intercepts gamepad MotionEvents to read
 * trigger and right-stick analog axes, forwarding them to Rust via JNI.
 *
 * Winit's GameActivity backend handles left stick and buttons natively.
 * This class adds the missing trigger (LT/RT) and right stick axes.
 */
public class FlintActivity extends GameActivity {

    static {
        System.loadLibrary("flint_android");
    }

    /**
     * JNI callback to send gamepad axis values to Rust.
     * Parameters: left trigger, right trigger, right stick X, right stick Y.
     * All values in [0, 1] for triggers, [-1, 1] for stick axes.
     */
    private static native void nativeOnGamepadAxes(
        float leftTrigger, float rightTrigger,
        float rightStickX, float rightStickY
    );

    @Override
    public boolean dispatchGenericMotionEvent(MotionEvent event) {
        int source = event.getSource();

        // Only intercept joystick or gamepad motion events
        if ((source & InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK
                || (source & InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD) {

            if (event.getAction() == MotionEvent.ACTION_MOVE) {
                // Read trigger axes — try standard AXIS_LTRIGGER/RTRIGGER first,
                // fall back to AXIS_BRAKE/AXIS_GAS for controllers that use those
                float lt = event.getAxisValue(MotionEvent.AXIS_LTRIGGER);
                float rt = event.getAxisValue(MotionEvent.AXIS_RTRIGGER);

                if (lt == 0.0f) {
                    lt = event.getAxisValue(MotionEvent.AXIS_BRAKE);
                }
                if (rt == 0.0f) {
                    rt = event.getAxisValue(MotionEvent.AXIS_GAS);
                }

                // Right stick: AXIS_Z (X) and AXIS_RZ (Y)
                float rsX = event.getAxisValue(MotionEvent.AXIS_Z);
                float rsY = event.getAxisValue(MotionEvent.AXIS_RZ);

                nativeOnGamepadAxes(lt, rt, rsX, rsY);
            }
        }

        // Always call super so GameActivity forwards events to native (winit)
        return super.dispatchGenericMotionEvent(event);
    }
}
