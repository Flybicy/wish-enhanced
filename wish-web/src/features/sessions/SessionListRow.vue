<script setup lang="ts">
import FadeText from '../../ui/components/FadeText.vue';
import Hint from '../../ui/components/Hint.vue';
import { computed } from 'vue';
import { DropdownMenuRoot, DropdownMenuTrigger, DropdownMenuPortal, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator } from 'reka-ui';
import { Tag } from '@lucide/vue';
import Icon from '../../ui/components/Icon.vue';
import { i18n } from '../../core/i18n/index.ts';
import { tr } from '../../core/i18n/tr.ts';
import { relTime } from '../../core/util/fmt.ts';
import { usePageActivity } from '../../ui/composables/usePageActivity.ts';
const props = defineProps<{ row: any; active: boolean; index: number }>();
const emit = defineEmits<{ action: [kind: 'rename' | 'tags' | 'delete' | 'pin' | 'unpin' | 'archive' | 'unarchive' | 'branch'] }>();
const pageActive = usePageActivity();
const name = computed(() => props.row.name || props.row.id.slice(0, 8));
const tags = computed<string[]>(() => Array.isArray(props.row.metadata?.tags) ? props.row.metadata.tags : []);
const pinned = computed(() => !!props.row.metadata?.pinned);
const archived = computed(() => !!props.row.metadata?.archived);
</script>

<template>
  <div class="sl-item" :class="{ active }" :data-session-id="row.id">
    <Hint :text="`${name} · ${relTime(row.updated_at_ms, i18n.t)}`"><RouterLink :to="`/s/${row.id}`" class="sl-row" :data-testid="`sl-row-${index}`"
      :aria-current="active ? 'page' : undefined">
      <Hint :text="i18n.t(`phase.${row.phase}`)"><span class="sl-status phase-dot" :class="row.phase" :aria-label="i18n.t(`phase.${row.phase}`)" /></Hint>
      <span class="sl-main">
        <FadeText class="sl-name" :text="name" />
        <Icon v-if="pinned" name="pin" class="sl-pin" />
        <span v-for="tag in tags" :key="tag" class="sl-tag">{{ tag }}</span>
      </span>
    </RouterLink></Hint>
    <DropdownMenuRoot :modal="false">
      <DropdownMenuTrigger class="sl-menu-trigger" :aria-label="`${i18n.t('manage.title')} · ${name}`"><Icon name="ellipsis-vertical" /></DropdownMenuTrigger>
      <DropdownMenuPortal v-if="pageActive">
        <DropdownMenuContent class="menu-pop sl-menu" align="end" :side-offset="4" :collision-padding="8">
          <DropdownMenuItem class="menu-item" @select="emit('action', 'rename')"><Icon name="pencil" />{{ i18n.t('manage.rename') }}</DropdownMenuItem>
          <DropdownMenuItem class="menu-item" @select="emit('action', 'tags')"><Tag class="icon" aria-hidden="true" />{{ i18n.t('sessions.editTags') }}</DropdownMenuItem>
          <DropdownMenuItem class="menu-item" @select="emit('action', 'branch')"><Icon name="git-branch" />{{ tr('分支会话', 'Branch') }}</DropdownMenuItem>
          <DropdownMenuSeparator class="sl-menu-separator" />
          <DropdownMenuItem class="menu-item" @select="emit('action', pinned ? 'unpin' : 'pin')"><Icon :name="pinned ? 'pin-off' : 'pin'" />{{ pinned ? tr('取消置顶', 'Unpin') : tr('置顶', 'Pin to top') }}</DropdownMenuItem>
          <DropdownMenuItem class="menu-item" @select="emit('action', archived ? 'unarchive' : 'archive')"><Icon :name="archived ? 'archive-restore' : 'archive'" />{{ archived ? tr('取消归档', 'Unarchive') : tr('归档', 'Archive') }}</DropdownMenuItem>
          <DropdownMenuSeparator class="sl-menu-separator" />
          <DropdownMenuItem class="menu-item sl-delete" @select="emit('action', 'delete')"><Icon name="trash-2" />{{ i18n.t('sessions.delete') }}</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenuPortal>
    </DropdownMenuRoot>
  </div>
</template>

<style scoped>
.sl-pin { width: 11px; height: 11px; flex: none; color: var(--accent); }
</style>
