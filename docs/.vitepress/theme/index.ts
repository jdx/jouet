import DefaultTheme from "vitepress/theme";
import type { Theme } from "vitepress";
import { h, onMounted, onUnmounted } from "vue";
import { data as starsData } from "../stars.data";
import EndevFooter from "./EndevFooter.vue";
import EndevSponsors from "./EndevSponsors.vue";
import HomeBeforeAfter from "./HomeBeforeAfter.vue";
import HomeConventions from "./HomeConventions.vue";
import HomeHeading from "./HomeHeading.vue";
import HomeQuickStart from "./HomeQuickStart.vue";
import HomeRobot from "./HomeRobot.vue";
import { initBanner } from "./banner";
import "./custom.css";

export default {
  extends: DefaultTheme,
  Layout() {
    return h(DefaultTheme.Layout, null, {
      "home-hero-image": () => h(HomeRobot),
      "home-hero-actions-after": () => h(HomeQuickStart),
      "home-features-before": () => [
        h(HomeBeforeAfter),
        h("div", { class: "jouet-section JouetCommandsHeading" }, [
          h("div", { class: "jouet-container" }, [
            h(
              HomeHeading,
              {
                id: "jouet-commands",
                eyebrow: "Commands",
                title: "One command per chore",
              },
              () =>
                "Each command replaces a script agents keep rewriting.",
            ),
          ]),
        ]),
      ],
      "home-features-after": () => h(HomeConventions),
      "layout-bottom": () => [h(EndevSponsors), h(EndevFooter)],
    });
  },
  enhanceApp() {
    initBanner();
  },
  setup() {
    let observer: MutationObserver | undefined;
    onMounted(() => {
      const addStarCount = () => {
        if (!starsData.stars) return false;

        const githubLinks = document.querySelectorAll(
          '.VPSocialLinks a[href*="github.com/jdx/jouet"]',
        );
        githubLinks.forEach((githubLink) => {
          if (!githubLink.querySelector(".star-count")) {
            const starBadge = document.createElement("span");
            starBadge.className = "star-count";
            starBadge.title = "GitHub Stars";
            const glyph = document.createElement("span");
            glyph.className = "star-glyph";
            glyph.textContent = "★";
            glyph.setAttribute("aria-hidden", "true");
            starBadge.append(glyph, starsData.stars);
            githubLink.appendChild(starBadge);
          }
        });
        return (
          githubLinks.length > 0 &&
          Array.from(githubLinks).every((link) =>
            link.querySelector(".star-count"),
          )
        );
      };

      addStarCount();
      observer = new MutationObserver(() => {
        addStarCount();
      });
      observer.observe(document.querySelector(".VPNav") || document.body, {
        childList: true,
        subtree: true,
      });
    });
    onUnmounted(() => observer?.disconnect());
  },
} satisfies Theme;
