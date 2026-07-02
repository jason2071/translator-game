import { create } from "zustand";

type Theme = "dark" | "light";
const KEY = "rpgtl.theme";

function apply(t: Theme) {
  document.documentElement.dataset.theme = t;
}

const initial: Theme = (localStorage.getItem(KEY) as Theme) || "dark";
apply(initial); // run at module load so the shell paints in the right theme

export const useTheme = create<{ theme: Theme; toggle: () => void }>((set, get) => ({
  theme: initial,
  toggle: () => {
    const t: Theme = get().theme === "dark" ? "light" : "dark";
    localStorage.setItem(KEY, t);
    apply(t);
    set({ theme: t });
  },
}));
