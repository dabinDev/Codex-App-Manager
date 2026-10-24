import type { ComponentType } from "react";

import type { IconName } from "../../icons";
import type { TKey } from "../../i18n";
import { ConversationAlignment } from "./ConversationAlignment";
import { UiLanguage } from "./UiLanguage";

export type ToolId = "conversation-alignment" | "ui-language";

export interface ToolDefinition {
  id: ToolId;
  icon: IconName;
  titleKey: TKey;
  statusKey: TKey;
  component: ComponentType;
}

export const TOOLS = [
  {
    id: "conversation-alignment",
    icon: "message",
    titleKey: "toolbox.alignment.title",
    statusKey: "toolbox.available",
    component: ConversationAlignment,
  },
  {
    id: "ui-language",
    icon: "globe",
    titleKey: "toolbox.locale.title",
    statusKey: "toolbox.available",
    component: UiLanguage,
  },
] as const satisfies readonly ToolDefinition[];
