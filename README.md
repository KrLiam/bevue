# About

Bevue brings [Vue](https://vuejs.org/guide/introduction.html)-like templating to Bevy UI, enabling fast prototyping of user interfaces while leveraging the power of Bevy's ECS. For that, it provides:
- An abstract UI node hierarchy to represent concepts such as interpolation, condition rendering and list rendering.
- A parser and asset loader to let Bevue templates be specified with Bevy assets.
- Lightweight (and very limited) scripting in template expressions, including the use of Rust functions in templates.
- A runtime engine that turn templates into concrete UI nodes with change-detection, immediate mode by default.

Bevue is only inspired by Vue, it won't:
- Support *all* syntax of Vue;
- Embed the Vue framework or a JavaScript engine. Everything still happens in Rust.

# Features

- [X] Vue-like components in HTML files;
- [X] Loading components from assets with hot reloading;
- [X] Easy styling with CSS through `bevy-flair` crate;
- [X] Rendering static components to Bevy UI;
- [ ] Text and attribute interpolation e.g `Hello {{ user }}!`;
- [ ] Conditional rendering using the `v-if` directive;
- [ ] List rendering using the `v-for` directive;