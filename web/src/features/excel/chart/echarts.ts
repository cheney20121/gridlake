/** 按需引入 ECharts：只注册用到的图表与组件，减小打包体积 */
import { BarChart, LineChart, PieChart, ScatterChart } from 'echarts/charts'
import { GridComponent, LegendComponent, TitleComponent, TooltipComponent } from 'echarts/components'
import * as echarts from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'

echarts.use([BarChart, LineChart, PieChart, ScatterChart, GridComponent, LegendComponent, TitleComponent, TooltipComponent, CanvasRenderer])

export { echarts }
