// UI strings (§9): every user-visible string lives in a message catalog and is
// formatted with ICU MessageFormat. English is the source locale; other
// catalogs must provide the same ids.

import { createIntl, createIntlCache, type IntlShape } from "@formatjs/intl";
import en from "./en.json";

export type MessageId = keyof typeof en;
type Values = Record<string, string | number | Date>;

const catalogs: Record<string, Record<MessageId, string>> = { en };

const cache = createIntlCache();
let intl: IntlShape = createIntl({ locale: "en", messages: en }, cache);

/** Switches the UI locale, falling back to English for unknown locales. */
export function setLocale(locale: string): void {
  const base = locale.split("-")[0] ?? "en";
  const messages = catalogs[locale] ?? catalogs[base] ?? en;
  intl = createIntl({ locale, defaultLocale: "en", messages }, cache);
}

/** Formats a UI string. */
export function t(id: MessageId, values?: Values): string {
  return intl.formatMessage({ id }, values);
}
