import i18n from 'i18next'
import { initReactI18next } from 'react-i18next'
import zh from './zh.json'
import en from './en.json'
import { getItem } from '../utils/storage'
import { STORAGE_KEYS } from '../constants'

export const LANG_STORAGE_KEY = STORAGE_KEYS.LANG

export function readSavedLang(): string {
  const v = getItem(STORAGE_KEYS.LANG)
  return v && v !== 'auto' ? v : 'zh'
}

i18n.use(initReactI18next).init({
  resources: {
    zh: { translation: zh },
    en: { translation: en },
  },
  lng: readSavedLang(),
  fallbackLng: 'zh',
  interpolation: {
    escapeValue: false,
  },
})

export default i18n
