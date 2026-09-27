// The shapes the browser client hands back, re-exported so the components import from one
// place, plus the `list/1` state this viewer knows how to draw as a checklist.

export type {
  AnomalyView,
  CandidateView,
  ChainView,
  EngagedView,
  Folder,
  FolderRole,
  LinkView,
  LoadedFile,
  OpenSeqView,
  PartyIndex,
  RecordView,
  SeatView,
  StatusView,
  SuspectView,
  WitnessTime,
} from "@synonymdev/mayfly-browser";

/** The `list/1` state, for the checklist rendering. */
export interface ListState {
  items: { id: string; text: string; qty?: number | null; ticked: boolean }[];
  archived: boolean;
  parties: number;
}
