import tailwindPlugin from "bun-plugin-tailwind";

const result = await Bun.build({
    entrypoints: ["./frontend.tsx", "./style.css"],
    outdir: "../",
    target: "browser",
    minify: true,
    naming: {
        entry: "[dir]/[name].[ext]",
    },
    plugins: [tailwindPlugin],
});

if (!result.success) {
    for (const log of result.logs) console.error(log);
    process.exit(1);
}

const fs = await import("node:fs/promises");
const path = await import("node:path");

for (const out of result.outputs) {
    if (out.path.endsWith("style.css")) {
        const target = path.join(path.dirname(out.path), "styles.gen.css");
        await fs.rename(out.path, target);
        console.log(`✓ ${target}`);
    } else {
        console.log(`✓ ${out.path}`);
    }
}
