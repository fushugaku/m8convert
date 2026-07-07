/* tslint:disable */
/* eslint-disable */

export class TeensyEmulator {
    free(): void;
    [Symbol.dispose](): void;
    audio_json(): string;
    clear_sd_image(): void;
    display_snapshot_json(): string;
    display_stats_json(): string;
    host_input_json(): string;
    load_sd_image(bytes: Uint8Array): void;
    constructor(input: Uint8Array);
    run_steps_json(max_steps: number): string;
    run_steps_live_json(max_steps: number): string;
    sd_card_json(): string;
    sd_image_bytes(): Uint8Array;
    send_host_bytes(bytes: Uint8Array): void;
    send_joypad_state(state: number): boolean;
    send_note_off(): void;
    send_note_on(note: number, velocity: number): void;
    take_audio_pcm_words(): Uint32Array;
    take_display_slip(): Uint8Array;
}

export function analyze_teensy_hex_json(input: Uint8Array): string;

export function convert_mod_to_m8_bundle_json(input: Uint8Array, song_name?: string | null): string;

export function convert_tracker_to_m8_bundle_json(input: Uint8Array, song_name?: string | null): string;

export function convert_xm_to_m8_bundle_json(input: Uint8Array, song_name?: string | null): string;

export function probe_teensy_hex_boot_json(input: Uint8Array, max_steps: number): string;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_teensyemulator_free: (a: number, b: number) => void;
    readonly analyze_teensy_hex_json: (a: number, b: number) => [number, number, number, number];
    readonly convert_mod_to_m8_bundle_json: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly convert_tracker_to_m8_bundle_json: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly convert_xm_to_m8_bundle_json: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly probe_teensy_hex_boot_json: (a: number, b: number, c: number) => [number, number, number, number];
    readonly teensyemulator_audio_json: (a: number) => [number, number, number, number];
    readonly teensyemulator_clear_sd_image: (a: number) => void;
    readonly teensyemulator_display_snapshot_json: (a: number) => [number, number, number, number];
    readonly teensyemulator_display_stats_json: (a: number) => [number, number, number, number];
    readonly teensyemulator_host_input_json: (a: number) => [number, number, number, number];
    readonly teensyemulator_load_sd_image: (a: number, b: number, c: number) => void;
    readonly teensyemulator_new: (a: number, b: number) => [number, number, number];
    readonly teensyemulator_run_steps_json: (a: number, b: number) => [number, number, number, number];
    readonly teensyemulator_run_steps_live_json: (a: number, b: number) => [number, number, number, number];
    readonly teensyemulator_sd_card_json: (a: number) => [number, number, number, number];
    readonly teensyemulator_sd_image_bytes: (a: number) => [number, number];
    readonly teensyemulator_send_host_bytes: (a: number, b: number, c: number) => void;
    readonly teensyemulator_send_joypad_state: (a: number, b: number) => number;
    readonly teensyemulator_send_note_off: (a: number) => void;
    readonly teensyemulator_send_note_on: (a: number, b: number, c: number) => void;
    readonly teensyemulator_take_audio_pcm_words: (a: number) => [number, number];
    readonly teensyemulator_take_display_slip: (a: number) => [number, number];
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
