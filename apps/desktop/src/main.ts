import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { createRouter, createWebHashHistory } from 'vue-router'
import 'element-plus/es/components/message/style/css'
import 'element-plus/es/components/message-box/style/css'
import './style.css'
import App from './App.vue'
import { useSession } from './stores/session'
const pinia = createPinia()
const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: '/login', component: () => import('./views/LoginView.vue') },
    {
      path: '/',
      component: () => import('./views/ShellLayout.vue'),
      children: [
        { path: 'profile', component: () => import('./views/ProfileView.vue') },
        { path: ':pathMatch(.*)*', component: () => import('./views/FeatureView.vue') },
      ],
    },
  ],
})
router.beforeEach(async (to) => {
  const session = useSession(pinia)
  await session.initialize()
  if (!session.user) return to.path === '/login' ? true : '/login'
  if (to.path === '/login' || to.path === '/') return session.firstPath
  if (to.path !== '/profile' && !session.findMenu(to.path)?.viewKey) return session.firstPath
  return true
})
createApp(App).use(pinia).use(router).mount('#app')
