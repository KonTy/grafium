<script lang="ts">
  import { mount, onMount, unmount } from "svelte";
  import ContextMenuView from "./ContextMenuView.svelte";

  // The menu is the app's only one, so it lives outside the page's own markup:
  // a node made here, in the body, that the view moves into whichever modal
  // dialog or popover a menu is about (they are drawn above everything else and
  // leave the rest of the page inert). It is made by script because Svelte
  // removes the nodes it rendered by walking from the first to the last, which
  // goes wrong if one of them has been moved somewhere else.
  onMount(() => {
    const container = document.createElement("div");
    container.className = "ctx-menu-container";
    container.style.display = "contents";
    document.body.append(container);
    const view = mount(ContextMenuView, { target: container, props: { container } });
    return () => {
      void unmount(view);
      container.remove();
    };
  });
</script>
