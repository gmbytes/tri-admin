<script setup lang="ts">
import { ref, watch } from 'vue'
import { useSession } from '../stores/session'
import { errorText } from '../ui'
const open = defineModel<boolean>({ required: true })
const session = useSession(),
  address = ref(''),
  error = ref('')
watch(open, (value) => {
  if (value) {
    address.value = session.settings.apiBaseUrl
    error.value = ''
  }
})
async function save() {
  error.value = ''
  try {
    await session.changeServer(address.value)
    open.value = false
  } catch (failure) {
    error.value = errorText(failure)
  }
}
</script>
<template>
  <el-dialog v-model="open" title="服务器设置" width="500px"
    ><p class="dialog-note">连接你的管理服务。切换服务器后需要重新登录。</p>
    <el-form label-position="top" @submit.prevent="save"
      ><el-form-item label="服务地址"
        ><el-input
          v-model="address"
          placeholder="https://admin.example.com"
          @keyup.enter="save" /></el-form-item
      ><el-alert v-if="error" :title="error" type="error" :closable="false" /></el-form
    ><template #footer
      ><el-button @click="open = false">取消</el-button
      ><el-button type="primary" :loading="session.busy" @click="save"
        >保存并连接</el-button
      ></template
    ></el-dialog
  >
</template>
