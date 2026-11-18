/**
 * `@elgato-stream-deck/core`'s `HIDDevice` over `ctx.hid`: the core does the
 * HID I/O, and the library drives the deck (plugin protocol spec §13.6).
 *
 * @module
 */

import type {
  ChildHIDDeviceInfo,
  HIDDevice,
  HIDDeviceEvents,
  HIDDeviceInfo,
} from "@elgato-stream-deck/core";
import { EventEmitter } from "eventemitter3";
import type { HidDevice } from "@covalence-io/sdk";

/** Stream Decks' input report ID. The library takes reports without it. */
const INPUT_REPORT = 0x01;

export class CoreHidDevice extends EventEmitter<HIDDeviceEvents>
  implements HIDDevice {
  /** When the core read the input report being handled, for `ts`. */
  lastInputTs: number | undefined;
  readonly #device: HidDevice;
  readonly #info: HIDDeviceInfo;

  constructor(device: HidDevice, info: HIDDeviceInfo) {
    super();
    this.#device = device;
    this.#info = info;
    device.onInput((report, ts) => {
      if (report[0] !== INPUT_REPORT) return;
      this.lastInputTs = ts;
      this.emit("input", report.subarray(1));
    });
  }

  close(): Promise<void> {
    this.#device.close();
    return Promise.resolve();
  }

  sendFeatureReport(data: Uint8Array): Promise<void> {
    return this.#device.sendFeatureReport(data);
  }

  getFeatureReport(
    reportId: number,
    reportLength: number,
  ): Promise<Uint8Array> {
    return this.#device.getFeatureReport(reportId, reportLength);
  }

  /** Output reports, their IDs first. The core writes them in order. */
  sendReports(buffers: Uint8Array[]): Promise<void> {
    for (const buffer of buffers) this.#device.write(buffer);
    return Promise.resolve();
  }

  getDeviceInfo(): Promise<HIDDeviceInfo> {
    return Promise.resolve(this.#info);
  }

  getChildDeviceInfo(): Promise<ChildHIDDeviceInfo | null> {
    return Promise.resolve(null);
  }
}
