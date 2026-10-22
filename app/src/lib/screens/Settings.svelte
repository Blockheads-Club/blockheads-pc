<script lang="ts">
  import { store } from '../store.svelte';
  import SettingsShell from '../layout/SettingsShell.svelte';
  import SettingsAbout from './SettingsAbout.svelte';
  import SettingsAudio from './SettingsAudio.svelte';
  import SettingsControls from './SettingsControls.svelte';
  import SettingsGame from './SettingsGame.svelte';
  import SettingsGraphics from './SettingsGraphics.svelte';
  import SettingsLanguage from './SettingsLanguage.svelte';
  import SettingsMods from './SettingsMods.svelte';
</script>

<!-- One page per sidebar item; only the selected page renders. -->
<SettingsShell>
  {#if store.section === 'gfx'}<SettingsGraphics />
  {:else if store.section === 'controls'}<SettingsControls />
  {:else if store.section === 'audio'}<SettingsAudio />
  {:else if store.section === 'lang'}<SettingsLanguage />
  {:else if store.section === 'mods'}<SettingsMods />
  {:else if store.section === 'about'}<SettingsAbout />
  {:else}<SettingsGame />{/if}
</SettingsShell>
