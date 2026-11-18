// A tab's validation rules (C1.15): what ranges of cells may hold, such as
// a list of choices that becomes a dropdown, or TRUE and FALSE as a checkbox.
import { useState } from "react";

import {
  Button,
  Checkbox,
  Dialog,
  IconButton,
  SelectField,
  TextField,
} from "../ui";
import type { SheetTab, ValidationRule } from "./api";

const KINDS: [ValidationRule["allow"], string][] = [
  ["list", "One of a list"],
  ["checkbox", "A checkbox"],
  ["number", "A number"],
  ["date", "A date"],
  ["textLength", "Text of a length"],
];

const asNumber = (text: string) =>
  text.trim() !== "" && Number.isFinite(Number(text))
    ? Number(text)
    : undefined;

/** Edits the rules on one tab, sent as one change when saved. */
export function SheetValidationDialog({
  onClose,
  onSave,
  selection,
  tab,
}: {
  onClose: () => void;
  onSave: (rules: ValidationRule[]) => Promise<boolean>;
  /** The selected cells, such as `C2:C40`, for a new rule's range. */
  selection: string;
  tab: SheetTab;
}) {
  const [rules, setRules] = useState<ValidationRule[]>(tab.validations ?? []);
  // Choices are edited as one line of text, split when saved.
  const [choices, setChoices] = useState<string[]>(() =>
    (tab.validations ?? []).map((rule) => (rule.choices ?? []).join(", ")),
  );
  const [busy, setBusy] = useState(false);
  const set = (index: number, change: Partial<ValidationRule>) =>
    setRules(
      rules.map((rule, at) => (at === index ? { ...rule, ...change } : rule)),
    );
  const setKind = (index: number, allow: ValidationRule["allow"]) => {
    const { range, strict, hint } = rules[index];
    setRules(
      rules.map((rule, at) =>
        at === index ? { range, allow, strict, hint } : rule,
      ),
    );
  };
  const save = async () => {
    setBusy(true);
    const saved = await onSave(
      rules.map((rule, index) =>
        rule.allow === "list"
          ? {
              ...rule,
              choices: (choices[index] ?? "")
                .split(",")
                .map((choice) => choice.trim())
                .filter(Boolean),
            }
          : rule,
      ),
    );
    setBusy(false);
    if (saved) onClose();
  };
  return (
    <Dialog
      description="Rules say what cells may hold. A strict rule refuses anything else; otherwise the cell is marked. A list gives its cells a dropdown, a checkbox a box."
      dirty
      footer={
        <>
          <Button onClick={onClose} variant="ghost">
            Cancel
          </Button>
          <Button busy={busy} type="submit" variant="primary">
            Save rules
          </Button>
        </>
      }
      onClose={onClose}
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
      open
      title={`Data validation: ${tab.name}`}
      width={680}
    >
      <div className="aw-sheet__view-filters">
        {rules.length === 0 ? (
          <p className="aw-sheet__rules-empty">No rules yet.</p>
        ) : null}
        {rules.map((rule, index) => (
          <div className="aw-sheet__validation" key={index}>
            <div className="aw-sheet__view-filter">
              <TextField
                aria-label="Cells"
                compact
                onChange={(event) =>
                  set(index, { range: event.target.value.toUpperCase() })
                }
                value={rule.range}
              />
              <SelectField
                aria-label="Allow"
                compact
                onChange={(event) =>
                  setKind(index, event.target.value as ValidationRule["allow"])
                }
                value={rule.allow}
              >
                {KINDS.map(([id, label]) => (
                  <option key={id} value={id}>
                    {label}
                  </option>
                ))}
              </SelectField>
              {rule.allow === "list" ? (
                <TextField
                  aria-label="Choices, separated by commas"
                  compact
                  onChange={(event) =>
                    setChoices((current) =>
                      Object.assign([...current], {
                        [index]: event.target.value,
                      }),
                    )
                  }
                  placeholder="Yes, No, Maybe"
                  value={choices[index] ?? ""}
                />
              ) : null}
              {rule.allow === "number" || rule.allow === "textLength" ? (
                <>
                  <TextField
                    aria-label="At least"
                    compact
                    inputMode="decimal"
                    onChange={(event) =>
                      set(index, { min: asNumber(event.target.value) })
                    }
                    placeholder="Min"
                    value={rule.min === undefined ? "" : String(rule.min)}
                  />
                  <TextField
                    aria-label="At most"
                    compact
                    inputMode="decimal"
                    onChange={(event) =>
                      set(index, { max: asNumber(event.target.value) })
                    }
                    placeholder="Max"
                    value={rule.max === undefined ? "" : String(rule.max)}
                  />
                </>
              ) : null}
              {rule.allow === "date" ? (
                <>
                  <TextField
                    aria-label="From"
                    compact
                    onChange={(event) =>
                      set(index, { from: event.target.value || undefined })
                    }
                    type="date"
                    value={rule.from ?? ""}
                  />
                  <TextField
                    aria-label="To"
                    compact
                    onChange={(event) =>
                      set(index, { to: event.target.value || undefined })
                    }
                    type="date"
                    value={rule.to ?? ""}
                  />
                </>
              ) : null}
              <IconButton
                icon="x"
                label="Remove this rule"
                onClick={() => {
                  setRules(rules.filter((_, at) => at !== index));
                  setChoices(choices.filter((_, at) => at !== index));
                }}
                size="sm"
              />
            </div>
            <div className="aw-sheet__view-filter">
              <TextField
                aria-label="Hint"
                compact
                onChange={(event) =>
                  set(index, { hint: event.target.value || undefined })
                }
                placeholder="Hint, such as “Pick a status”"
                value={rule.hint ?? ""}
              />
              {rule.allow === "number" ? (
                <Checkbox
                  checked={!!rule.whole}
                  label="Whole numbers"
                  onChange={(checked) =>
                    set(index, { whole: checked || undefined })
                  }
                />
              ) : null}
              <Checkbox
                checked={rule.strict !== false}
                label="Refuse anything else"
                onChange={(checked) =>
                  set(index, { strict: checked ? undefined : false })
                }
              />
            </div>
          </div>
        ))}
        <Button
          icon="plus"
          onClick={() => {
            setRules([...rules, { range: selection, allow: "list" }]);
            setChoices([...choices, ""]);
          }}
          size="sm"
          variant="ghost"
        >
          Add a rule
        </Button>
      </div>
    </Dialog>
  );
}
