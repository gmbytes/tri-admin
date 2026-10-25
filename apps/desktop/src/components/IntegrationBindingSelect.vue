<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useSession } from '../stores/session'
import { errorText } from '../ui'
import { integrationLabel, type IntegrationKind, type IntegrationProfile } from '../core/settings'
const props = defineProps<{ kinds: IntegrationKind[]; disabled?: boolean }>()
const model = defineModel<string | null>({ required: true })
const session = useSession(),
  rows = ref<IntegrationProfile[]>([]),
  loading = ref(false),
  error = ref('')
const selected = computed({
  get: () => model.value ?? ':none:',
  set: (value) => {
    model.value = value === ':none:' ? null : value
  },
})
let sequence = 0
async function load(search = '') {
  const request = ++sequence,
    pinnedId = model.value
  loading.value = true
  error.value = ''
  try {
    const results = await Promise.all(
      props.kinds.map((kind) =>
        session.request<IntegrationProfile[]>(
          `/settings/profiles?${new URLSearchParams({ kind, pageSize: '100', sort: 'code', search })}`,
        ),
      ),
    )
    const candidates = results.flatMap((result) => result.data)
    if (pinnedId && !candidates.some((row) => row.id === pinnedId)) {
      candidates.unshift(
        (await session.request<IntegrationProfile>(`/settings/profiles/${pinnedId}`)).data,
      )
    }
    if (request === sequence) rows.value = candidates
  } catch (failure) {
    if (request === sequence) error.value = errorText(failure)
  } finally {
    if (request === sequence) loading.value = false
  }
}
onMounted(() => void load())
</script>
<template>
  <el-select
    v-model="selected"
    filterable
    remote
    :remote-method="load"
    :loading="loading"
    :disabled="disabled"
    placeholder="按配置标识前缀搜索"
  >
    <el-option value=":none:" label="不绑定" />
    <el-option
      v-for="row in rows"
      :key="row.id"
      :value="row.id"
      :disabled="!row.enabled"
      :label="`${row.name} · ${row.code} · ${integrationLabel(row.configuration.kind)}`"
    />
  </el-select>
  <small v-if="error" class="field-error">{{ error }}</small>
</template>
