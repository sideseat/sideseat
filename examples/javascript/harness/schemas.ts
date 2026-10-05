/** The shared trip-plan schema, in the shapes the frameworks take it. */
import { z } from 'zod';
import { TRIP_PLAN_SCHEMA, type TripPlan } from './content.js';

/** The trip plan as a Zod schema, converted from the Python harness's JSON schema. */
export function tripPlan(): z.ZodType<TripPlan> {
  const schema = TRIP_PLAN_SCHEMA as Parameters<typeof z.fromJSONSchema>[0];
  return z.fromJSONSchema(schema) as z.ZodType<TripPlan>;
}

/**
 * The trip plan as a tool the model may choose to hand its plan to.
 *
 * Bedrock rejects native structured output (`output_config.format`) and a forced tool choice for
 * current Claude models, so where a framework would use either, the schema is offered as this tool
 * instead. It answers, so the conversation stays balanced: every call has its result.
 */
export function planTool() {
  const plans: TripPlan[] = [];
  return {
    name: 'trip_plan',
    description: 'Record the finished trip plan.',
    instruction: 'Hand the finished plan to trip_plan.',
    parameters: TRIP_PLAN_SCHEMA,
    record(input: unknown): string {
      plans.push(tripPlan().parse(input));
      return 'Plan recorded.';
    },
    recorded(): TripPlan {
      const plan = plans.at(-1);
      if (!plan) throw new Error('the model recorded no trip plan');
      return plan;
    },
  };
}
