// Small decorative SVGs from the "Bohemian dusk" design (design/UI-SPEC.md).
// All are aria-hidden and drawn in currentColor unless noted.

/** Eight-ray starburst (the ✳ mark). */
export const starburst = (size = 12, cls = '') =>
  `<svg class="orn ${cls}" width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><path d="M12 2v20M2 12h20M4.9 4.9l14.2 14.2M19.1 4.9L4.9 19.1"/></svg>`;

/** Cross with a dot on each side, used beside wordmarks and dividers. */
export const sparkle = (cls = '') =>
  `<svg class="orn sparkle ${cls}" width="22" height="14" viewBox="0 0 22 14" fill="none" stroke="currentColor" stroke-width="1.2" aria-hidden="true"><path d="M11 0v14M4 7h14"/><circle cx="1.5" cy="7" r="1.2" fill="currentColor"/><circle cx="20.5" cy="7" r="1.2" fill="currentColor"/></svg>`;

/** A filled and an open dot. */
export const dots = (filledFirst = true) =>
  `<span class="dots" aria-hidden="true"><span class="${filledFirst ? 'on' : ''}"></span><span class="${filledFirst ? '' : 'on'}"></span></span>`;

/** Checkerboard strip. */
export const checker = () =>
  `<svg class="orn" width="44" height="10" viewBox="0 0 40 10" fill="currentColor" aria-hidden="true"><rect x="0" y="0" width="5" height="5"/><rect x="10" y="0" width="5" height="5"/><rect x="20" y="0" width="5" height="5"/><rect x="30" y="0" width="5" height="5"/><rect x="5" y="5" width="5" height="5"/><rect x="15" y="5" width="5" height="5"/><rect x="25" y="5" width="5" height="5"/><rect x="35" y="5" width="5" height="5"/></svg>`;

/** Atom orbit. */
export const orbit = () =>
  `<svg class="orn" width="34" height="16" viewBox="0 0 34 16" fill="none" stroke="currentColor" stroke-width="1.1" aria-hidden="true"><ellipse cx="17" cy="8" rx="15" ry="6"/><path d="M17 1v14M10 8h14"/></svg>`;

/** Little pennant above the wordmark. */
export const pennant = () =>
  `<svg class="orn" width="12" height="10" viewBox="0 0 12 10" aria-hidden="true"><path d="M0 0h12L6 4z M6 4v6" fill="currentColor" stroke="currentColor"/></svg>`;

/** Fills every [data-orn] placeholder in the static markup. */
export function fillOrnaments(root: ParentNode = document) {
  root.querySelectorAll<HTMLElement>('[data-orn]').forEach((el) => {
    const kind = el.dataset.orn;
    el.outerHTML =
      kind === 'starburst'
        ? starburst(Number(el.dataset.size) || 14, el.className)
        : kind === 'sparkle'
          ? sparkle(el.className)
          : kind === 'dots'
            ? dots(el.dataset.first !== 'open')
            : kind === 'checker'
              ? checker()
              : kind === 'orbit'
                ? orbit()
                : kind === 'pennant'
                  ? pennant()
                  : '';
  });
}
