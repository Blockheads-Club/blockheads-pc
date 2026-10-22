import type { IconName } from '../icons';
import type { Key } from '../messages';

/** 0 low, 1 caution, 2 high, 'native' danger (design: permission tiers). */
export type Tier = 0 | 1 | 2 | 'native';

export interface PermInfo {
  /** i18n stem: `perm.<key>`, `perm.<key>.title`, `perm.<key>.desc`. */
  key: string;
  tier: Tier;
  icon: IconName;
  /** `net-http:<host>`: the host. */
  host?: string;
}

const TABLE: Record<string, Omit<PermInfo, 'host'>> = {
  'world:read': { key: 'worldRead', tier: 0, icon: 'eye' },
  'world:write': { key: 'worldWrite', tier: 1, icon: 'pencil' },
  'players:chat': { key: 'chat', tier: 0, icon: 'chat' },
  'players:teleport': { key: 'teleport', tier: 1, icon: 'tele' },
  'players:kick': { key: 'kick', tier: 2, icon: 'kick' },
  'players:ban': { key: 'ban', tier: 2, icon: 'ban' },
  'players:inventory': { key: 'inventory', tier: 1, icon: 'bag' },
  'players:health': { key: 'health', tier: 1, icon: 'heart' },
  channels: { key: 'channels', tier: 0, icon: 'radio' },
  native: { key: 'native', tier: 'native', icon: 'octagon' }
};

/** What a permission name (as in mods.toml) means for the UI. Unknown names show as caution with their raw name. */
export function permInfo(name: string): PermInfo {
  if (name.startsWith('net-http:')) return { key: 'net', tier: 1, icon: 'globe', host: name.slice(9) };
  return TABLE[name] ?? { key: 'other', tier: 1, icon: 'key' };
}

/** i18n key of one of the permission's texts. */
export function permKey(info: PermInfo, part: '' | '.title' | '.desc'): Key {
  return `perm.${info.key}${part}` as Key;
}
