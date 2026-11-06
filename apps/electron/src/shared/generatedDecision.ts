// Generated from harness-runtime/crates/decision-api/src/lib.rs. Do not edit.
export interface DecisionModelRef { backend: string; account: string; model: string }
export interface DecisionImage { data_url: string }
export type DecisionQuestion =
  | { type: 'binary'; instructions: string }
  | { type: 'choice'; instructions: string; options: Record<string, string> }
  | { type: 'score'; instructions: string; levels: Array<string> };
export type DecisionAnswer =
  | { type: 'refused'; reason: string | null }
  | { type: 'binary'; probability: number }
  | { type: 'choice'; option: string; probabilities: Record<string, number> | null; confidence: number | null }
  | { type: 'score'; value: number; probabilities: Array<number> | null; confidence: number | null };
export interface DecisionRequest { model: DecisionModelRef; state_revision: number; state: unknown; images?: Array<DecisionImage>; questions: Record<string, DecisionQuestion> }
export interface DecisionUsage { input_tokens: number | null; output_tokens: number | null }
export interface DecisionResponse { state_revision: number; answers: Record<string, DecisionAnswer>; usage: DecisionUsage; actual_model: string | null; elapsed_ms: number }
