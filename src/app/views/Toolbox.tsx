import { useState } from "react";

import { NavBar } from "../components";
import { Icon } from "../icons";
import { useI18n } from "../i18n";
import { TOOLS, type ToolId } from "./toolbox/registry";

export function Toolbox({ onBack }: { onBack: () => void }) {
  const { t } = useI18n();
  const [selected, setSelected] = useState<ToolId | null>(null);
  const selectedTool = TOOLS.find((tool) => tool.id === selected) ?? null;

  if (selectedTool) {
    const Detail = selectedTool.component;
    return (
      <div className="pop">
        <NavBar title={t(selectedTool.titleKey)} onBack={() => setSelected(null)} />
        <div className="scroll scroll-wide view" data-tool-detail={selectedTool.id}>
          <Detail />
        </div>
      </div>
    );
  }

  return (
    <div className="pop">
      <NavBar title={t("toolbox.title")} onBack={onBack} />
      <div className="scroll view">
        <section className="group" aria-labelledby="toolbox-heading">
          <h1 className="group-h" id="toolbox-heading">
            {t("toolbox.title")}
          </h1>
          <div className="list toolbox-list">
            {TOOLS.map((tool) => (
              <button
                type="button"
                className="row"
                key={tool.id}
                onClick={() => setSelected(tool.id)}
              >
                <Icon name={tool.icon} className="ricon" />
                <span className="rtext">
                  <span className="rtitle">{t(tool.titleKey)}</span>
                </span>
                <span className="rval">{t(tool.statusKey)}</span>
                <Icon name="chevron" className="chev" />
              </button>
            ))}
          </div>
        </section>
      </div>
    </div>
  );
}
