/**
 * Method tables: which methods exist, in which direction, with which params
 * and results. Mirrors `crates/covalence-protocol/src/methods.rs`.
 *
 * @module
 */

import type * as T from "./types.gen.ts";

/** Requests a host sends to the core. */
export interface HostToCoreRequests {
  "host.hello": { params: T.HostHello; result: T.HostWelcome };
  ping: { params: T.Empty; result: T.Empty };
  "io.open": { params: T.IoOpen; result: T.IoOpened };
  "io.feature": { params: T.IoFeature; result: T.IoFeatureResult };
  "device.list": { params: T.DeviceList; result: T.DeviceInfoList };
}

/** Notifications a host sends to the core. */
export interface HostToCoreNotifications {
  "conn.status": T.ConnStatus;
  "state.snapshot": T.StateSnapshot;
  "state.patch": T.StatePatch;
  "event.emit": T.EventEmit;
  log: T.Log;
  "io.write": T.IoWrite;
  "io.close": T.IoClose;
  "surface.attach": T.SurfaceAttach;
  "surface.detach": T.SurfaceDetach;
  "surface.input": T.SurfaceInput;
}

/** Requests the core sends to a host. */
export interface CoreToHostRequests {
  "plugin.load": { params: T.PluginLoad; result: T.PluginLoaded };
  "conn.create": { params: T.ConnCreate; result: T.Empty };
  "conn.destroy": { params: T.ConnDestroy; result: T.Empty };
  "conn.resync": { params: T.Empty; result: T.Empty };
  "action.invoke": { params: T.ActionInvoke; result: T.ActionResult };
  ping: { params: T.Empty; result: T.Empty };
}

/** Notifications the core sends to a host. */
export interface CoreToHostNotifications {
  "io.data": T.IoData;
  "io.state": T.IoState;
  "surface.draw": T.SurfaceDraw;
}
