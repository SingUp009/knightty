// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import react from "@astrojs/react";

// https://astro.build/config
export default defineConfig({
  integrations: [
    starlight({
      title: "Knightty",
      sidebar: [
        {
          label: "Start",
          items: [{ label: "Overview", link: "/" }],
        },
        {
          label: "Reference",
          items: [{ label: "Config Reference", link: "/config/reference/" }],
        },
      ],
    }),
    react(),
  ],
});
