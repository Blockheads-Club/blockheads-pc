<script lang="ts">
  import { api } from '../api';
  import { t } from '../i18n.svelte';
  import { store } from '../store.svelte';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';

  /** The page's own title (parsing runs no scripts), else a plain one. */
  const title = $derived(new DOMParser().parseFromString(store.page, 'text/html').title.trim() || t('page.title'));

  /** The game's welcome page hides the world details behind a button with an inline script (scripts never run in
      the frame): the same toggle, done from here. */
  function spoiler(doc: Document, button: Element) {
    const box = doc.getElementById('spoiler');
    if (!box) return;
    const el = button as HTMLElement;
    el.dataset.label ??= el.textContent!;
    const open = box.style.display === 'none';
    box.style.display = open ? '' : 'none';
    el.textContent = open ? 'Hide' : el.dataset.label;
  }

  /** Links leave for the browser: the frame runs no scripts and never navigates. */
  function links(e: Event) {
    // Same origin (sandbox), so the document is reachable. Elements come from the frame's realm: no instanceof.
    const doc = (e.currentTarget as HTMLIFrameElement).contentDocument!;
    doc.addEventListener('click', (ev) => {
      const button = (ev.target as Element).closest?.('#spoilerButton');
      if (button) return spoiler(doc, button);
      const a = (ev.target as Element).closest?.('a[href]');
      if (!a) return;
      ev.preventDefault();
      const href = a.getAttribute('href')!;
      if (/^(https?:|mailto:)/i.test(href)) void api.openUrl(href).catch(() => {});
    });
  }
</script>

<Dialog {title} wide onclose={() => store.closePage()}>
  <!-- allow-same-origin without allow-scripts: the page cannot run code; it only lets the links be handled here. -->
  <iframe class="page" title={title} sandbox="allow-same-origin" srcdoc={store.page} onload={links}></iframe>
  {#snippet footer()}
    <Button variant="primary" onclick={() => store.closePage()}>{t('page.back')}</Button>
  {/snippet}
</Dialog>

<style>
  .page { display: block; width: 100%; height: min(60vh, 560px); border: 0; border-radius: 8px; background: #fff; }
</style>
