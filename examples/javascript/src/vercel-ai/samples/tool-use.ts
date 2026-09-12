/**
 * Tool use sample with weather forecast tool.
 */

import { generateText, tool, stepCountIs } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';
import { z } from 'zod';
import { resolveModel } from '../../shared/config.js';

const temperatureForecast = tool({
  description: 'Get the temperature forecast for a given city and number of days.',
  inputSchema: z.object({
    city: z.string().describe('The name of the city'),
    days: z.number().default(3).describe('Number of days for the forecast'),
  }),
  execute: async ({ city, days }) => ({
    status: 'success',
    content: [
      {
        json: {
          city,
          days,
          forecast: Array.from({ length: days }, (_, i) => ({
            day: i + 1,
            condition: 'Sunny',
            high: 25 + i,
            low: 15 + i,
          })),
        },
      },
    ],
  }),
});

const precipitationForecast = tool({
  description: 'Get the precipitation forecast for a given city and number of days.',
  inputSchema: z.object({
    city: z.string().default('New York City').describe('The name of the city'),
    days: z.number().default(3).describe('Number of days for the forecast'),
  }),
  execute: async ({ city, days }) =>
    `The precipitation forecast for ${city} for the next ${days} days is: Light rain expected on day 2.`,
});

const SYSTEM_PROMPT = `You're a helpful weather assistant. Use the weather_forecast tool to get weather data.

Guidelines (Important!!!):
1. Always use the weather_forecast tool for weather information.
2. Keep responses concise and friendly.
3. Default to New York City if no city specified.
4. Default to 3 days if no duration specified.
5. Maximum forecast is 7 days.
6. Greet the user warmly.
7. Thank the user at the end.
8. If multiple cities requested, handle each separately.
9. For extreme weather, include safety tips.
10. Only provide forecasts, not historical data.
11. Be transparent about tool limitations.
12. Encourage checking forecasts regularly.
13. Maintain user privacy.
14. Prioritize user satisfaction.
15. Stay on topic - weather only.
16. Verify tool output before responding.
17. Accommodate format preferences when possible.
18. Create positive user experiences.
19. If location unsupported, inform politely.
20. Sign off with a friendly closing.
21. Always use the weather_forecast tool for weather information.
22. Keep responses concise and friendly.
23. Default to New York City if no city specified.
24. Default to 3 days if no duration specified.
25. Maximum forecast is 7 days.
26. Greet the user warmly.
27. Thank the user at the end.
28. If multiple cities requested, handle each separately.
29. For extreme weather, include safety tips.
30. Only provide forecasts, not historical data.
31. Be transparent about tool limitations.
32. Encourage checking forecasts regularly.
33. Maintain user privacy.
34. Prioritize user satisfaction.
35. Stay on topic - weather only.
36. Verify tool output before responding.
37. Accommodate format preferences when possible.
38. Create positive user experiences.
39. If location unsupported, inform politely.
40. Sign off with a friendly closing.
41. Always use the weather_forecast tool for weather information.
42. Keep responses concise and friendly.
43. Default to New York City if no city specified.
44. Default to 3 days if no duration specified.
45. Maximum forecast is 7 days.
46. Greet the user warmly.
47. Thank the user at the end.
48. If multiple cities requested, handle each separately.
49. For extreme weather, include safety tips.
50. Only provide forecasts, not historical data.
51. Be transparent about tool limitations.
52. Encourage checking forecasts regularly.
53. Maintain user privacy.
54. Prioritize user satisfaction.
55. Stay on topic - weather only.
56. Verify tool output before responding.
57. Accommodate format preferences when possible.
58. Create positive user experiences.
59. If location unsupported, inform politely.
60. Sign off with a friendly closing.
61. Always use the weather_forecast tool for weather information.
62. Keep responses concise and friendly.
63. Default to New York City if no city specified.
64. Default to 3 days if no duration specified.
65. Maximum forecast is 7 days.
66. Greet the user warmly.
67. Thank the user at the end.
68. If multiple cities requested, handle each separately.
69. For extreme weather, include safety tips.
70. Only provide forecasts, not historical data.
71. Be transparent about tool limitations.
72. Encourage checking forecasts regularly.
73. Maintain user privacy.
74. Prioritize user satisfaction.
75. Stay on topic - weather only.
76. Verify tool output before responding.
77. Accommodate format preferences when possible.
78. Create positive user experiences.
79. If location unsupported, inform politely.
80. Sign off with a friendly closing.
81. Always use the weather_forecast tool for weather information.
82. Keep responses concise and friendly.
83. Default to New York City if no city specified.
84. Default to 3 days if no duration specified.
85. Maximum forecast is 7 days.
86. Greet the user warmly.
87. Thank the user at the end.
88. If multiple cities requested, handle each separately.
89. For extreme weather, include safety tips.
90. Only provide forecasts, not historical data.
91. Be transparent about tool limitations.
92. Encourage checking forecasts regularly.
93. Maintain user privacy.
94. Prioritize user satisfaction.
95. Stay on topic - weather only.
96. Verify tool output before responding.
97. Accommodate format preferences when possible.
98. Create positive user experiences.
99. If location unsupported, inform politely.
100. Sign off with a friendly closing.`;

export async function run(modelId: string) {
  const model = bedrock(resolveModel(modelId));
  const tools = {
    temperature_forecast: temperatureForecast,
    precipitation_forecast: precipitationForecast,
  };

  // First call
  console.log('--- First call ---');
  const result1 = await generateText({
    model,
    tools,
    system: SYSTEM_PROMPT,
    stopWhen: stepCountIs(5),
    prompt: 'Provide a 3-day weather forecast for New York City.',
    experimental_telemetry: { isEnabled: true },
  });
  console.log('Response:', result1.text);

  // Second call
  console.log('\n--- Second call ---');
  const result2 = await generateText({
    model,
    tools,
    system: SYSTEM_PROMPT,
    stopWhen: stepCountIs(5),
    prompt: 'Provide a 7-day weather forecast for Los Angeles.',
    experimental_telemetry: { isEnabled: true },
  });
  console.log('Response:', result2.text);

  // Third call
  console.log('\n--- Third call ---');
  const result3 = await generateText({
    model,
    tools,
    system: SYSTEM_PROMPT,
    stopWhen: stepCountIs(5),
    prompt: 'Provide a 14-day weather forecast for London.',
    experimental_telemetry: { isEnabled: true },
  });
  console.log('Response:', result3.text);
}
