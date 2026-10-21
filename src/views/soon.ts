import { h } from "../dom";
import { header, View } from "./types";

export function soonView(title: string, line: string, step: string): View {
  return {
    mount(root) {
      root.append(header(title), h("div", { class: "soon" }, h("h2", { text: `${title} arrives in ${step}` }), h("p", { text: line })));
    },
  };
}
