import nodeResolve from "@rollup/plugin-node-resolve";
import commonjs from "@rollup/plugin-commonjs";
import typescript from "@rollup/plugin-typescript";

export default {
    input: "src/plugin.ts",
    output: {
        file: "com.micro-emu.codex.sdPlugin/bin/plugin.js",
        format: "esm",
        sourcemap: false,
    },
    plugins: [
        nodeResolve({ preferBuiltins: true }),
        commonjs(),
        typescript({ tsconfig: "./tsconfig.json" }),
    ],
};
