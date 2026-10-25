<script setup lang="ts">
import { ref, watch, onMounted, onBeforeUnmount, computed } from 'vue'
import { useSession } from '../stores/session'
import type { IntegrationProfile } from '../core/settings'
const props = defineProps<{ modelValue: string | null }>(),
  emit = defineEmits<{ 'update:modelValue': [value: string | null] }>(),
  session = useSession(),
  rows = ref<IntegrationProfile[]>([]),
  selected = ref<IntegrationProfile | null>(null),
  loading = ref(false)
let sequence = 0,
  alive = true
const choices = computed(() =>
  selected.value && !rows.value.some((row) => row.id === selected.value!.id)
    ? [selected.value, ...rows.value]
    : rows.value,
)
async function load(search = '') {
  const seq = ++sequence
  loading.value = true
  try {
    const result = await Promise.all(
      ['localStorage', 's3'].map((kind) =>
        session.request<IntegrationProfile[]>(
          `/settings/profiles?${new URLSearchParams({ pageSize: '100', sort: 'code', kind, search })}`,
        ),
      ),
    )
    if (alive && seq === sequence)
      rows.value = result.flatMap((value) => value.data).filter((row) => row.enabled)
  } finally {
    if (alive && seq === sequence) loading.value = false
  }
}
watch(
  () => props.modelValue,
  async (id) => {
    if (!id) {
      selected.value = null
      return
    }
    try {
      const row = (await session.request<IntegrationProfile>(`/settings/profiles/${id}`)).data
      if (alive && props.modelValue === id) selected.value = row
    } catch {
      /* Save-time validation remains authoritative. */
    }
  },
  { immediate: true },
)
onMounted(() => {
  void load().catch(() => undefined)
})
onBeforeUnmount(() => {
  alive = false
  sequence++
})
</script>
<template>
  <el-select
    :model-value="modelValue"
    clearable
    filterable
    remote
    :remote-method="(search: string) => load(search).catch(() => undefined)"
    :loading="loading"
    placeholder="使用服务默认存储"
    @update:model-value="emit('update:modelValue', $event || null)"
    ><el-option
      v-for="row in choices"
      :key="row.id"
      :value="row.id"
      :label="`${row.name}（${row.code}）`"
  /></el-select>
</template>
