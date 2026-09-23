import { useState } from "react";

interface OpenState {
  local: boolean;
  lastForce: boolean | undefined;
}

export function useForcedOpenState(forceExpanded: boolean | undefined, initialOpen = true) {
  const [state, setState] = useState<OpenState>({
    local: forceExpanded ?? initialOpen,
    lastForce: forceExpanded,
  });

  if (state.lastForce !== forceExpanded) {
    setState({
      local: forceExpanded ?? state.local,
      lastForce: forceExpanded,
    });
  }

  const setOpen = (open: boolean) => {
    setState({ local: open, lastForce: forceExpanded });
  };

  return [forceExpanded ?? state.local, setOpen] as const;
}
