import { h } from "preact";
import htm from "htm";

// Shared htm-bound template function for every module.
export const html = htm.bind(h);
