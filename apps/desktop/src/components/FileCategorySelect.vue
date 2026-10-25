<script setup lang="ts">
import { ref, watch, onMounted, onBeforeUnmount, computed } from 'vue'
import { useSession } from '../stores/session'
import type { FileCategory } from '../core/file-models'
defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ modelValue: string | null; disabled?: boolean }>(), {
    disabled: false,
  }),
  emit = defineEmits<{ 'update:modelValue': [value: string | null] }>(),
  session = useSession()
const rows = ref<FileCategory[]>([]),
  selected = ref<FileCategory | null>(null),
  loading = ref(false),
  error = ref('')
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
  error.value = ''
  try {
    const result = await session.request<FileCategory[]>(
      `/file-categories?${new URLSearchParams({ pageSize: '100', sort: 'code', search })}`,
    )
    if (alive && seq === sequence) rows.value = result.data
  } catch {
    if (alive && seq === sequence) error.value = '分类选项读取失败，可重新搜索'
  } finally {
    if (alive && seq === sequence) loading.value = false
  }
}
async function keep(id: string | null) {
  if (!id) {
    selected.value = null
    return
  }
  try {
    const row = (await session.request<FileCategory>(`/file-categories/${id}`)).data
    if (alive && props.modelValue === id) selected.value = row
  } catch {
    if (alive && props.modelValue === id)
      selected.value = {
        id,
        name: '分类已不可用',
        code: id,
        description: '',
        version: 0,
        createdAt: '',
        updatedAt: '',
      }
  }
}
watch(() => props.modelValue, keep, { immediate: true })
onMounted(() => load())
onBeforeUnmount(() => {
  alive = false
  sequence++
})
</script>
<template>
  <el-select
    v-bind="$attrs"
    :model-value="props.modelValue"
    :disabled="disabled"
    clearable
    filterable
    remote
    :remote-method="load"
    :loading="loading"
    placeholder="未分类；可按标识搜索"
    @update:model-value="emit('update:modelValue', $event || null)"
  >
    <el-option
      v-for="row in choices"
      :key="row.id"
      :value="row.id"
      :label="`${row.name}（${row.code}）`"
    /> </el-select
  ><small v-if="error" class="field-error">{{ error }}</small>
</template>
