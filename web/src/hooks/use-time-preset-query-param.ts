import { useEffect, useState } from "react";
import { StringParam, useQueryParam, withDefault } from "use-query-params";

import { DEFAULT_TIME_PRESET } from "@/lib/time-filter";
import { GLOBAL_TIME_PRESET_KEY, settings } from "@/lib/settings";

export function useTimePresetQueryParam() {
  const [defaultPreset] = useState(
    () => settings.get<string>(GLOBAL_TIME_PRESET_KEY) ?? DEFAULT_TIME_PRESET,
  );
  const queryParam = useQueryParam("time", withDefault(StringParam, defaultPreset));
  const [timePreset] = queryParam;

  useEffect(() => {
    if (timePreset) {
      settings.set(GLOBAL_TIME_PRESET_KEY, timePreset);
    }
  }, [timePreset]);

  return queryParam;
}
