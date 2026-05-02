---
"@biomejs/biome": patch
---

Added the new nursery rule [`noReactDeps`](https://biomejs.dev/linter/rules/no-react-deps/) for Solid projects.

The rule reports passing a dependency array as the second argument to `createEffect` or `createMemo`,
which is a React idiom that has no effect in Solid. The second argument is used as the initial value
passed to the function, not as a dependency list.

```js
import { createEffect } from "solid-js";
createEffect(() => {
  console.log(signal());
}, [signal]);
```
