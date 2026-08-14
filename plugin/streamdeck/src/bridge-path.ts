import { existsSync, statSync } from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

/** Resolve the installed or bundled bridge executable for Stream Deck. */
export function resolveBridgeExecutable(
    moduleUrl: string = import.meta.url,
    environment: NodeJS.ProcessEnv = process.env,
): string | undefined {
    const pluginBin = path.dirname(fileURLToPath(moduleUrl));
    const candidates = [
        environment.MICRO_EMU_BRIDGE_EXE,
        environment.LOCALAPPDATA
            ? path.join(environment.LOCALAPPDATA, "micro-emu", "rp2040-bridge.exe")
            : undefined,
        path.resolve(pluginBin, "..", "..", "rp2040-bridge.exe"),
        path.resolve(pluginBin, "..", "..", "..", "..", "tools", "rp2040-bridge", "target", "release", "rp2040-bridge.exe"),
        path.resolve(pluginBin, "..", "..", "..", "..", "tools", "rp2040-bridge", "target-next", "release", "rp2040-bridge.exe"),
        path.resolve(pluginBin, "..", "..", "..", "..", "artifacts", "cargo-target", "release", "rp2040-bridge.exe"),
    ];

    const seen = new Set<string>();
    for (const candidate of candidates) {
        if (!candidate) continue;
        const resolved = path.resolve(candidate);
        const key = process.platform === "win32" ? resolved.toLowerCase() : resolved;
        if (seen.has(key)) continue;
        seen.add(key);
        try {
            if (existsSync(resolved) && statSync(resolved).isFile()) return resolved;
        } catch {
            // Continue past stale or inaccessible candidates.
        }
    }
    return undefined;
}
