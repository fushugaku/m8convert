const params = new URLSearchParams(self.location.search);
const buildStamp = params.get("v") || "dev";

let moduleReady = null;
let m8Module = null;
let emulator = null;
let messageQueue = Promise.resolve();

function ensureModule() {
  if (!moduleReady) {
    moduleReady = (async () => {
      m8Module = await import(`./pkg/m8convert.js?v=${buildStamp}`);
      await m8Module.default({ module_or_path: `./pkg/m8convert_bg.wasm?v=${buildStamp}` });
    })();
  }
  return moduleReady;
}

function concatBytes(parts) {
  const total = parts.reduce((sum, part) => sum + (part?.length || 0), 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    if (!part?.length) continue;
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function parseJson(value) {
  return JSON.parse(value);
}

function requireEmulator() {
  if (!emulator) {
    throw new Error("Live emulator is not initialized");
  }
  return emulator;
}

function numberField(value, fallback = 0) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

function hostInputPacketMatches(hostInput, state) {
  const packet = hostInput?.last_packet || [];
  return packet.length >= 2
    && numberField(packet[0], -1) === 0x43
    && numberField(packet[1], -1) === state;
}

function joypadInputApplied(before, after, state) {
  if (!hostInputPacketMatches(after, state)) return false;

  const expectedFirmwareState = (~state) & 0xff;
  const remoteControllerMatches =
    numberField(after?.observed_m8_remote_controller_current, -1) === state
    && (
      numberField(after?.remote_controller_consume_successes) > numberField(before?.remote_controller_consume_successes)
      || numberField(after?.remote_serial_pump_successes) > numberField(before?.remote_serial_pump_successes)
    );
  const controllerHandlerMatches =
    numberField(after?.observed_m8_controller_state, -1) === state
    || numberField(after?.observed_m8_controller_state, -1) === expectedFirmwareState;
  const inputMemoryMatches =
    numberField(after?.input_memory_current_state, -1) === expectedFirmwareState
    && numberField(after?.input_memory_dirty_flag, 0) !== 0;

  return remoteControllerMatches || controllerHandlerMatches || inputMemoryMatches;
}

function runSteps(maxSteps, live) {
  const instance = requireEmulator();
  const summary = parseJson(
    live && instance.run_steps_live_json
      ? instance.run_steps_live_json(maxSteps)
      : instance.run_steps_json(maxSteps),
  );
  const displaySlip = instance.take_display_slip();
  summary.display = parseJson(instance.display_stats_json());
  return {
    summary,
    displaySlip,
    audioPcmWords: instance.take_audio_pcm_words(),
  };
}

function hostState() {
  const instance = requireEmulator();
  return {
    hostInput: parseJson(instance.host_input_json()),
    sdCard: parseJson(instance.sd_card_json()),
    audio: parseJson(instance.audio_json()),
    displaySlip: instance.take_display_slip(),
    display: parseJson(instance.display_stats_json()),
    audioPcmWords: instance.take_audio_pcm_words(),
  };
}

async function handleMessage(event) {
  const { id, type, ...payload } = event.data || {};
  const transfers = [];

  try {
    await ensureModule();
    let result = {};

    switch (type) {
      case "create": {
        if (emulator?.free) {
          emulator.free();
        }
        emulator = new m8Module.TeensyEmulator(payload.firmwareBytes);
        if (payload.sdImageBytes?.length) {
          emulator.load_sd_image(payload.sdImageBytes);
        }
        emulator.send_host_bytes(new Uint8Array([0x45, 0x52]));
        const before = emulator.take_display_slip();
        const first = runSteps(1, false);
        const displaySlip = concatBytes([before, first.displaySlip]);
        result = {
          summary: first.summary,
          displaySlip,
          audioPcmWords: first.audioPcmWords,
          sdCard: parseJson(emulator.sd_card_json()),
          hostInput: parseJson(emulator.host_input_json()),
          audio: parseJson(emulator.audio_json()),
        };
        transfers.push(displaySlip.buffer);
        transfers.push(result.audioPcmWords.buffer);
        break;
      }
      case "run": {
        result = runSteps(payload.steps, Boolean(payload.live));
        transfers.push(result.displaySlip.buffer);
        transfers.push(result.audioPcmWords.buffer);
        break;
      }
      case "sendHostBytes": {
        requireEmulator().send_host_bytes(payload.bytes);
        result = hostState();
        transfers.push(result.displaySlip.buffer);
        transfers.push(result.audioPcmWords.buffer);
        break;
      }
      case "sendJoypadState": {
        const instance = requireEmulator();
        const state = Number(payload.state || 0) & 0xff;
        const beforeHostInput = parseJson(instance.host_input_json());
        const directInputApplied = instance.send_joypad_state(state);
        if (!directInputApplied) {
          instance.send_host_bytes(new Uint8Array([0x43, state]));
        }
        const steps = Number(payload.steps || 0);
        if (steps > 0) {
          result = runSteps(steps, true);
        } else {
          result = {
            summary: null,
            displaySlip: instance.take_display_slip(),
            audioPcmWords: instance.take_audio_pcm_words(),
          };
        }
        result.hostInput = parseJson(instance.host_input_json());
        result.sdCard = parseJson(instance.sd_card_json());
        result.audio = parseJson(instance.audio_json());
        result.display = parseJson(instance.display_stats_json());
        if (result.summary) {
          result.summary.display = result.display;
        }
        result.inputApplied = directInputApplied || joypadInputApplied(beforeHostInput, result.hostInput, state);
        transfers.push(result.displaySlip.buffer);
        transfers.push(result.audioPcmWords.buffer);
        break;
      }
      case "loadSdImage": {
        requireEmulator().load_sd_image(payload.bytes);
        result = hostState();
        transfers.push(result.displaySlip.buffer);
        transfers.push(result.audioPcmWords.buffer);
        break;
      }
      case "snapshot": {
        result = {
          snapshot: parseJson(requireEmulator().display_snapshot_json()),
        };
        break;
      }
      case "sdImage": {
        const bytes = requireEmulator().sd_image_bytes();
        result = { bytes };
        transfers.push(bytes.buffer);
        break;
      }
      case "dispose": {
        if (emulator?.free) {
          emulator.free();
        }
        emulator = null;
        result = { disposed: true };
        break;
      }
      default:
        throw new Error(`Unknown emulator worker message: ${type}`);
    }

    self.postMessage({ id, ok: true, result }, transfers);
  } catch (error) {
    self.postMessage({
      id,
      ok: false,
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

self.onmessage = (event) => {
  messageQueue = messageQueue
    .catch(() => {})
    .then(() => handleMessage(event));
};
